"""Comprehensive offline CPython 3.12 parity tests for `socketserver` (issue #3223)."""

import contextlib
import http.server
import io
import socket
import socketserver
import sys

# ---------------------------------------------------------------------------
# 1. Module exports, version, and class hierarchy
# ---------------------------------------------------------------------------
assert isinstance(socketserver.__version__, str)
for name in [
    "BaseServer",
    "TCPServer",
    "UDPServer",
    "ThreadingMixIn",
    "ThreadingTCPServer",
    "ThreadingUDPServer",
    "BaseRequestHandler",
    "StreamRequestHandler",
    "DatagramRequestHandler",
]:
    assert name in socketserver.__all__, name
    assert hasattr(socketserver, name), name

assert issubclass(socketserver.TCPServer, socketserver.BaseServer)
assert issubclass(socketserver.UDPServer, socketserver.TCPServer)
assert issubclass(socketserver.ThreadingTCPServer, socketserver.ThreadingMixIn)
assert issubclass(socketserver.ThreadingTCPServer, socketserver.TCPServer)
assert issubclass(socketserver.ThreadingUDPServer, socketserver.ThreadingMixIn)
assert issubclass(socketserver.ThreadingUDPServer, socketserver.UDPServer)
assert issubclass(socketserver.StreamRequestHandler, socketserver.BaseRequestHandler)
assert issubclass(socketserver.DatagramRequestHandler, socketserver.BaseRequestHandler)

# Compatibility with http.server
assert issubclass(http.server.HTTPServer, socketserver.TCPServer)
assert issubclass(http.server.HTTPServer, socketserver.BaseServer)
assert issubclass(http.server.ThreadingHTTPServer, socketserver.ThreadingMixIn)
assert issubclass(http.server.ThreadingHTTPServer, http.server.HTTPServer)
assert issubclass(http.server.BaseHTTPRequestHandler, socketserver.StreamRequestHandler)

# Class defaults
assert socketserver.BaseServer.timeout is None
assert socketserver.TCPServer.address_family == socket.AF_INET
assert socketserver.TCPServer.socket_type == socket.SOCK_STREAM
assert socketserver.TCPServer.request_queue_size == 5
assert socketserver.TCPServer.allow_reuse_address is False
assert socketserver.UDPServer.socket_type == socket.SOCK_DGRAM
assert socketserver.UDPServer.max_packet_size == 8192

print("hierarchy_and_defaults_ok")

# ---------------------------------------------------------------------------
# 2. BaseRequestHandler lifecycle (normal & exception in handle)
# ---------------------------------------------------------------------------
events = []


class LifecycleHandler(socketserver.BaseRequestHandler):
    def setup(self):
        events.append(("setup", self.request, self.client_address, self.server))

    def handle(self):
        events.append(("handle", self.request))
        if self.request == "boom":
            raise RuntimeError("handler error")

    def finish(self):
        events.append(("finish", self.request))


LifecycleHandler("req1", ("127.0.0.1", 1000), "srv")
assert events == [
    ("setup", "req1", ("127.0.0.1", 1000), "srv"),
    ("handle", "req1"),
    ("finish", "req1"),
]

events.clear()
try:
    LifecycleHandler("boom", ("127.0.0.1", 1001), "srv")
    assert False, "expected RuntimeError"
except RuntimeError as exc:
    assert str(exc) == "handler error"
assert events == [
    ("setup", "boom", ("127.0.0.1", 1001), "srv"),
    ("handle", "boom"),
    ("finish", "boom"),
]

print("base_handler_lifecycle_ok")

# ---------------------------------------------------------------------------
# 3. TCPServer + StreamRequestHandler loopback (handle_request, verify_request,
#    handle_error, timeout/handle_timeout, bind_and_activate=False, serve_forever)
# ---------------------------------------------------------------------------
class EchoStreamHandler(socketserver.StreamRequestHandler):
    def handle(self):
        line = self.rfile.readline()
        if line == b"error\n":
            raise ValueError("intentional handler failure")
        if line == b"stop\n":
            self.server._stop_flag = True
        self.wfile.write(b"ECHO:" + line)


class BufferedStreamHandler(socketserver.StreamRequestHandler):
    wbufsize = -1

    def handle(self):
        line = self.rfile.readline()
        self.wfile.write(b"BUF:" + line)


class _StopServe(Exception):
    pass


class TestTCPServer(socketserver.TCPServer):
    allow_reuse_address = True

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.reject_next = False
        self.timed_out = 0
        self._stop_flag = False

    def verify_request(self, request, client_address):
        if self.reject_next:
            self.reject_next = False
            return False
        return True

    def handle_timeout(self):
        self.timed_out += 1

    def service_actions(self):
        if self._stop_flag:
            raise _StopServe()


# bind_and_activate=False + manual server_bind / server_activate + context manager
srv_manual = TestTCPServer(("127.0.0.1", 0), EchoStreamHandler, bind_and_activate=False)
srv_manual.server_bind()
srv_manual.server_activate()
with srv_manual as srv:
    assert isinstance(srv, socketserver.TCPServer)
    assert isinstance(srv, socketserver.BaseServer)
    assert srv.fileno() >= 0
    host, port = srv.server_address
    assert host == "127.0.0.1" and port > 0

    # 3a. Normal TCP echo via handle_request
    with socket.create_connection(("127.0.0.1", port), timeout=5) as cli:
        cli.sendall(b"hello tcp\n")
        srv.handle_request()
        reply = cli.recv(1024)
        assert reply == b"ECHO:hello tcp\n", reply

    # 3b. verify_request returning False closes connection without calling handler
    srv.reject_next = True
    with socket.create_connection(("127.0.0.1", port), timeout=5) as cli:
        cli.sendall(b"ignored\n")
        srv.handle_request()
        reply = cli.recv(1024)
        assert reply == b"", reply

    # 3c. handle_error catches handler Exception and logs traceback to stderr
    err_buf = io.StringIO()
    with socket.create_connection(("127.0.0.1", port), timeout=5) as cli:
        cli.sendall(b"error\n")
        with contextlib.redirect_stderr(err_buf):
            srv.handle_request()
    err_text = err_buf.getvalue()
    assert "Exception occurred during processing of request from" in err_text
    assert "ValueError: intentional handler failure" in err_text

    # Server still healthy after handler error
    with socket.create_connection(("127.0.0.1", port), timeout=5) as cli:
        cli.sendall(b"after-error\n")
        srv.handle_request()
        assert cli.recv(1024) == b"ECHO:after-error\n"

    # 3d. handle_request timeout triggers handle_timeout
    srv.timeout = 0.03
    srv.handle_request()
    assert srv.timed_out == 1
    srv.timeout = None

    # 3e. serve_forever + service_actions + shutdown
    with socket.create_connection(("127.0.0.1", port), timeout=5) as cli:
        cli.sendall(b"stop\n")
        try:
            srv.serve_forever(poll_interval=0.02)
        except _StopServe:
            pass
        srv.shutdown()
        assert cli.recv(1024) == b"ECHO:stop\n"

# Context manager closed the listening socket
assert srv_manual.fileno() == -1

# Buffered StreamRequestHandler (wbufsize = -1)
with socketserver.TCPServer(("127.0.0.1", 0), BufferedStreamHandler) as bsrv:
    _, bport = bsrv.server_address
    with socket.create_connection(("127.0.0.1", bport), timeout=5) as cli:
        cli.sendall(b"buffered\n")
        bsrv.handle_request()
        assert cli.recv(1024) == b"BUF:buffered\n"

# Verify HTTPServer instance is also a socketserver.TCPServer
with http.server.HTTPServer(("127.0.0.1", 0), http.server.BaseHTTPRequestHandler) as hsrv:
    assert isinstance(hsrv, socketserver.TCPServer)
    assert isinstance(hsrv, socketserver.BaseServer)

print("tcp_server_ok")

# ---------------------------------------------------------------------------
# 4. UDPServer + DatagramRequestHandler loopback
# ---------------------------------------------------------------------------
class EchoDatagramHandler(socketserver.DatagramRequestHandler):
    def handle(self):
        data = self.rfile.read()
        assert data == self.packet
        assert self.socket is self.server.socket
        self.wfile.write(b"UDP:" + data.upper())


with socketserver.UDPServer(("127.0.0.1", 0), EchoDatagramHandler) as usrv:
    assert isinstance(usrv, socketserver.UDPServer)
    assert isinstance(usrv, socketserver.TCPServer)
    assert usrv.fileno() >= 0
    uaddr = usrv.server_address
    assert uaddr[0] == "127.0.0.1" and uaddr[1] > 0

    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as ucli:
        ucli.settimeout(5)
        ucli.sendto(b"ping-dgram", uaddr)
        usrv.handle_request()
        resp, peer = ucli.recvfrom(1024)
        assert resp == b"UDP:PING-DGRAM", resp
        assert peer == uaddr

assert usrv.fileno() == -1
print("udp_server_ok")
print("socketserver_all_ok")
