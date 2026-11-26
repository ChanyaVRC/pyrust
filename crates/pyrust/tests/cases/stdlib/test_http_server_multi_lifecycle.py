"""Sequential HTTP server lifecycles and GIL-releasing socket waits (issue #3325).

A second `HTTPServer.serve_forever()` in the same process used to hang: the
serving thread polled `selectors` (and `select`, `connect`, `send*`,
`recv_into`, `recvfrom`, ...) while still holding the GIL, starving the client
thread, so even `urlopen(..., timeout=3)` could not fire.  `ThreadingMixIn`
was also a synchronous stub, so `ThreadingHTTPServer` served one request at a
time.

Each "GIL released" probe below parks a worker thread in one blocking call
with a 10 s timeout and makes the main thread complete the wait.  When the
call releases the GIL (CPython) the probe finishes at once; when it holds the
GIL the main thread cannot run until the timeout, so the probe reports the
timeout instead and the output differs.
"""

import http.server
import select
import selectors
import socket
import socketserver
import threading
import urllib.request

PROBE_TIMEOUT = 10.0


class HiHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"hi" + self.path.encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def serve_in_thread(server, poll_interval=0.05):
    thread = threading.Thread(
        target=server.serve_forever, args=(poll_interval,), daemon=True
    )
    thread.start()
    return thread


def stop(server, thread):
    server.shutdown()
    server.server_close()
    thread.join(timeout=PROBE_TIMEOUT)
    return thread.is_alive()


def fetch(port, path="/", timeout=3):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=timeout) as resp:
        return resp.status, resp.read()


def tcp_pair():
    """Return a connected `(a, b)` pair of TCP sockets on loopback."""
    with socket.socket() as lsock:
        lsock.bind(("127.0.0.1", 0))
        lsock.listen(1)
        a = socket.create_connection(lsock.getsockname(), timeout=PROBE_TIMEOUT)
        b, _ = lsock.accept()
    a.settimeout(None)
    return a, b


def gil_probe(label, wait, complete):
    """Run `wait()` in a worker; once it is about to block, run `complete()`."""
    entering = threading.Event()
    result = []

    def worker():
        entering.set()
        try:
            result.append(wait())
        except OSError as exc:
            result.append(type(exc).__name__)

    thread = threading.Thread(target=worker)
    thread.start()
    entering.wait(PROBE_TIMEOUT)
    complete()
    thread.join(PROBE_TIMEOUT)
    print(f"{label}: {result[0] if result else 'no result'}")


# 1. The issue reproducer: several HTTPServer lifecycles in one process, with
#    serve_forever()'s default 0.5 s poll interval.
print("== sequential HTTPServer ==")
for i in range(3):
    srv = http.server.HTTPServer(("127.0.0.1", 0), HiHandler)
    port = srv.server_address[1]
    t = serve_in_thread(srv, poll_interval=0.5)
    print(i, fetch(port), "alive_after_stop:", stop(srv, t))

# 2. The same for ThreadingHTTPServer, which now spawns a thread per request.
print("== sequential ThreadingHTTPServer ==")
for i in range(3):
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), HiHandler)
    port = srv.server_address[1]
    t = serve_in_thread(srv)
    print(i, fetch(port, f"/{i}"), type(srv._threads).__name__)
    print(i, "alive_after_stop:", stop(srv, t))

# 3. ThreadingHTTPServer serves requests concurrently: /a can only finish
#    once /b has been handled on another thread.
print("== concurrent ThreadingHTTPServer requests ==")
a_entered = threading.Event()
b_seen = threading.Event()


class RendezvousHandler(HiHandler):
    def do_GET(self):
        if self.path == "/a":
            a_entered.set()
            self.server.a_saw_b = b_seen.wait(PROBE_TIMEOUT)
        else:
            b_seen.set()
        super().do_GET()


srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), RendezvousHandler)
srv.a_saw_b = None
port = srv.server_address[1]
t = serve_in_thread(srv)
results = {}
clients = [
    threading.Thread(target=lambda p=p: results.__setitem__(p, fetch(port, p, timeout=PROBE_TIMEOUT)))
    for p in ("/a", "/b")
]
clients[0].start()
# /b is only requested once /a's handler is parked, so it can only be served
# by a second request thread.
a_entered.wait(PROBE_TIMEOUT)
clients[1].start()
for c in clients:
    c.join(PROBE_TIMEOUT)
print(sorted(results.items()), "a_saw_b:", srv.a_saw_b)
print("alive_after_stop:", stop(srv, t))

# 4. Many client threads against one ThreadingHTTPServer.
print("== parallel clients ==")
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), HiHandler)
port = srv.server_address[1]
t = serve_in_thread(srv)
bodies = []
lock = threading.Lock()


def client(n):
    status, body = fetch(port, f"/c{n}", timeout=PROBE_TIMEOUT)
    with lock:
        bodies.append((status, body))


workers = [threading.Thread(target=client, args=(n,)) for n in range(6)]
for w in workers:
    w.start()
for w in workers:
    w.join(PROBE_TIMEOUT)
print(sorted(bodies))
print("alive_after_stop:", stop(srv, t))

# 5. ThreadingTCPServer (non-daemon threads, block_on_close) joins its
#    request threads in server_close().
print("== ThreadingTCPServer block_on_close ==")
handling = threading.Event()
release = threading.Event()
finished = []


class SlowEcho(socketserver.StreamRequestHandler):
    def handle(self):
        line = self.rfile.readline()
        handling.set()
        release.wait(PROBE_TIMEOUT)
        self.wfile.write(line.upper())
        finished.append(line.strip())


srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), SlowEcho)
port = srv.server_address[1]
t = serve_in_thread(srv)
with socket.create_connection(("127.0.0.1", port), timeout=PROBE_TIMEOUT) as cli:
    cli.sendall(b"ping\n")
    handling.wait(PROBE_TIMEOUT)
    srv.shutdown()
    print("daemon_threads:", srv.daemon_threads, "tracked:", len(srv._threads))
    release.set()
    srv.server_close()
    print("finished before close returned:", finished, "tracked:", len(srv._threads))
    print("reply:", cli.recv(64))
t.join(PROBE_TIMEOUT)

# 6. Client timeouts fire while a serving thread is busy elsewhere.
print("== client timeout honoured ==")
unblock = threading.Event()


class StallHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        unblock.wait(PROBE_TIMEOUT)

    def log_message(self, *args):
        pass


srv = http.server.HTTPServer(("127.0.0.1", 0), StallHandler)
port = srv.server_address[1]
t = serve_in_thread(srv)
try:
    fetch(port, timeout=0.5)
except TimeoutError as exc:
    print("urlopen:", type(exc).__name__, exc)
unblock.set()
print("alive_after_stop:", stop(srv, t))

with socket.socket() as silent:
    silent.bind(("127.0.0.1", 0))
    silent.listen(8)
    outcomes = []

    def silent_client():
        with socket.create_connection(silent.getsockname(), timeout=0.3) as c:
            try:
                c.recv(1)
                outcome = "data"
            except TimeoutError as exc:
                outcome = f"{type(exc).__name__}: {exc}"
        with lock:
            outcomes.append(outcome)

    waiters = [threading.Thread(target=silent_client) for _ in range(4)]
    for w in waiters:
        w.start()
    for w in waiters:
        w.join(PROBE_TIMEOUT)
    print("concurrent recv timeouts:", sorted(outcomes))

# 7. Blocking waits release the GIL.
print("== GIL released while blocked ==")
a, b = tcp_pair()
gil_probe(
    "select.select",
    lambda: len(select.select([a], [], [], PROBE_TIMEOUT)[0]),
    lambda: b.sendall(b"x"),
)
a.recv(1)

with selectors.DefaultSelector() as sel:
    sel.register(a, selectors.EVENT_READ)
    gil_probe(
        "selectors",
        lambda: [key.fileobj is a for key, _ in sel.select(PROBE_TIMEOUT)],
        lambda: b.sendall(b"y"),
    )
a.recv(1)

a.settimeout(PROBE_TIMEOUT)
gil_probe("recv", lambda: a.recv(16), lambda: b.sendall(b"recv"))
buf = bytearray(16)
gil_probe(
    "recv_into",
    lambda: bytes(buf[: a.recv_into(buf)]),
    lambda: b.sendall(b"recv_into"),
)

# sendall of more data than the socket buffers hold completes only while a
# reader on another thread drains the peer.
payload = b"z" * (16 * 1024 * 1024)
received = []


def drain():
    total = 0
    while total < len(payload):
        chunk = b.recv(1 << 16)
        if not chunk:
            break
        total += len(chunk)
    received.append(total)


reader = threading.Thread(target=drain)
reader.start()
try:
    a.sendall(payload)
    print("sendall: sent", len(payload))
except OSError as exc:
    print("sendall:", type(exc).__name__)
reader.join(PROBE_TIMEOUT)
print("sendall: peer received", received)
a.close()
b.close()

with socket.socket() as lsock:
    lsock.bind(("127.0.0.1", 0))
    lsock.listen(1)
    lsock.settimeout(PROBE_TIMEOUT)
    conns = []

    def connect_to_listener():
        conns.append(socket.create_connection(lsock.getsockname(), timeout=PROBE_TIMEOUT))

    gil_probe(
        "accept",
        lambda: type(lsock.accept()[0]).__name__,
        connect_to_listener,
    )
    for c in conns:
        c.close()

with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as rx, socket.socket(
    socket.AF_INET, socket.SOCK_DGRAM
) as tx:
    rx.bind(("127.0.0.1", 0))
    rx.settimeout(PROBE_TIMEOUT)
    gil_probe(
        "recvfrom",
        lambda: rx.recvfrom(64)[0],
        lambda: tx.sendto(b"datagram", rx.getsockname()),
    )

print("done")
