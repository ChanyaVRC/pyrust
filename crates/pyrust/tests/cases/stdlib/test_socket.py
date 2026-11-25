import socket

# 1. Constants, enums, exceptions
assert issubclass(socket.AddressFamily, int)
assert issubclass(socket.SocketKind, int)
assert socket.AF_INET == socket.AddressFamily.AF_INET
assert socket.AF_INET6 == socket.AddressFamily.AF_INET6
assert socket.AF_UNSPEC == socket.AddressFamily.AF_UNSPEC
assert socket.SOCK_STREAM == socket.SocketKind.SOCK_STREAM
assert socket.SOCK_DGRAM == socket.SocketKind.SOCK_DGRAM
assert isinstance(socket.SOL_SOCKET, int)
assert isinstance(socket.SO_REUSEADDR, int)
assert isinstance(socket.IPPROTO_TCP, int)
assert isinstance(socket.IPPROTO_UDP, int)
assert isinstance(socket.TCP_NODELAY, int)
assert (socket.SHUT_RD, socket.SHUT_WR, socket.SHUT_RDWR) == (0, 1, 2)
assert socket.SOMAXCONN > 0
assert isinstance(socket.AI_PASSIVE, int)
assert isinstance(socket.AI_CANONNAME, int)
assert isinstance(socket.AI_NUMERICHOST, int)
assert socket.has_ipv6 is True
assert socket.error is OSError
assert socket.timeout is TimeoutError
assert issubclass(socket.gaierror, OSError)
assert issubclass(socket.herror, OSError)
print("constants_and_exceptions_ok")

# 2. Byte-order and address helpers
assert socket.ntohs(socket.htons(80)) == 80
assert socket.ntohs(socket.htons(0x1234)) == 0x1234
assert socket.ntohl(socket.htonl(0x12345678)) == 0x12345678
assert socket.htons(0) == 0
assert socket.htonl(0) == 0

for fn, bad in [
    (socket.htons, -1),
    (socket.htons, 65536),
    (socket.htonl, -1),
    (socket.htonl, 2**32),
]:
    try:
        fn(bad)
        assert False, f"expected OverflowError for {fn.__name__}({bad})"
    except OverflowError:
        pass

assert socket.inet_aton("127.0.0.1") == b"\x7f\x00\x00\x01"
assert socket.inet_aton("127.1") == b"\x7f\x00\x00\x01"
assert socket.inet_ntoa(b"\x7f\x00\x00\x01") == "127.0.0.1"
assert socket.inet_pton(socket.AF_INET, "192.0.2.1") == b"\xc0\x00\x02\x01"
assert socket.inet_ntop(socket.AF_INET, b"\xc0\x00\x02\x01") == "192.0.2.1"
packed_v6 = socket.inet_pton(socket.AF_INET6, "2001:0db8:0000:0000:0000:0000:0000:0001")
assert len(packed_v6) == 16
assert socket.inet_ntop(socket.AF_INET6, packed_v6) == "2001:db8::1"

for bad_v4 in ["999.0.0.1", "not_an_ip"]:
    try:
        socket.inet_pton(socket.AF_INET, bad_v4)
        assert False
    except OSError:
        pass

try:
    socket.inet_ntop(socket.AF_INET, b"\x01\x02")
    assert False
except ValueError:
    pass
print("addr_helpers_ok")

# 3. Hostname, default timeout, getaddrinfo (100% offline numeric/loopback)
hn = socket.gethostname()
assert isinstance(hn, str) and len(hn) > 0
fqdn = socket.getfqdn("127.0.0.1")
assert isinstance(fqdn, str) and len(fqdn) > 0
assert socket.gethostbyname("127.0.0.1") == "127.0.0.1"

assert socket.getdefaulttimeout() is None
socket.setdefaulttimeout(3.5)
assert socket.getdefaulttimeout() == 3.5
s_def = socket.socket()
assert s_def.gettimeout() == 3.5
assert s_def.getblocking() is True
s_def.close()
socket.setdefaulttimeout(None)
assert socket.getdefaulttimeout() is None

ai = socket.getaddrinfo("127.0.0.1", 8080, socket.AF_INET, socket.SOCK_STREAM)
assert len(ai) >= 1
fam, kind, proto, cname, sa = ai[0]
assert fam == socket.AF_INET
assert kind == socket.SOCK_STREAM
assert isinstance(proto, int)
assert isinstance(cname, str)
assert sa == ("127.0.0.1", 8080)

ai_passive = socket.getaddrinfo(
    None, 9000, socket.AF_INET, socket.SOCK_STREAM, 0, socket.AI_PASSIVE
)
assert ai_passive[0][4] == ("0.0.0.0", 9000)

try:
    socket.getaddrinfo("not-a-numeric-ip", 80, socket.AF_INET, socket.SOCK_STREAM, 0, socket.AI_NUMERICHOST)
    assert False
except socket.gaierror:
    pass
print("dns_and_default_timeout_ok")

# 4. TCP loopback: bind/listen/accept/connect/send/sendall/recv/recv_into/shutdown/opts
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as srv:
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    srv.listen(5)
    host, port = srv.getsockname()
    assert host == "127.0.0.1" and port > 0
    assert srv.family == socket.AF_INET
    assert srv.type == socket.SOCK_STREAM
    assert srv.fileno() >= 0

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as cli:
        cli.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        assert cli.getsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY) != 0
        rc = cli.connect_ex(("127.0.0.1", port))
        assert rc == 0
        assert cli.getpeername() == ("127.0.0.1", port)

        conn, peer = srv.accept()
        with conn:
            assert peer[0] == "127.0.0.1"
            assert peer == cli.getsockname()
            assert conn.getpeername() == peer

            # send / recv
            sent = cli.send(b"hello ")
            assert sent == 6
            cli.sendall(b"world!")
            buf = b""
            while len(buf) < 12:
                chunk = conn.recv(64)
                assert chunk
                buf += chunk
            print("tcp_recv:", buf)

            # recv_into with bytearray and memoryview slice
            conn.sendall(b"ABCDEFGH")
            ba = bytearray(4)
            n1 = cli.recv_into(ba)
            assert n1 > 0
            received = bytes(ba[:n1])
            while len(received) < 8:
                mv_buf = bytearray(8)
                mv = memoryview(mv_buf)
                n2 = cli.recv_into(mv, 8 - len(received))
                assert n2 > 0
                received += bytes(mv_buf[:n2])
            print("tcp_recv_into:", received)

            # shutdown write signals EOF to peer
            cli.shutdown(socket.SHUT_WR)
            eof = conn.recv(16)
            assert eof == b""

    assert cli.fileno() == -1
assert srv.fileno() == -1
print("tcp_loopback_ok")

# 5. Timeouts & non-blocking mode
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as srv:
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    srv.settimeout(0.05)
    assert srv.gettimeout() == 0.05
    assert srv.getblocking() is True
    try:
        srv.accept()
        assert False, "expected TimeoutError on accept"
    except TimeoutError:
        pass

    srv.setblocking(False)
    assert srv.gettimeout() == 0.0
    assert srv.getblocking() is False
    try:
        srv.accept()
        assert False, "expected BlockingIOError on nonblocking accept"
    except BlockingIOError:
        pass

    srv.settimeout(None)
    assert srv.gettimeout() is None
    assert srv.getblocking() is True

    # Recv timeout on connected stream
    cli = socket.create_connection(srv.getsockname(), timeout=2.0)
    conn, _ = srv.accept()
    conn.settimeout(0.05)
    try:
        conn.recv(16)
        assert False, "expected TimeoutError on recv"
    except TimeoutError:
        pass
    conn.close()
    cli.close()
print("timeouts_ok")

# 6. create_server + create_connection + makefile (binary & text, plus _io_refs)
with socket.create_server(("127.0.0.1", 0)) as srv:
    addr = srv.getsockname()
    cli = socket.create_connection(addr, timeout=2.0)
    conn, _ = srv.accept()

    # Binary buffered makefile + _io_refs keeps socket alive after sock.close()
    wfile = cli.makefile("wb")
    rfile = conn.makefile("rb")
    # Close client socket while wfile still holds a reference
    cli.close()
    wfile.write(b"GET / HTTP/1.1\r\n")
    wfile.write(b"Host: localhost\r\n\r\n")
    wfile.flush()
    wfile.close()

    line1 = rfile.readline()
    line2 = rfile.readline()
    line3 = rfile.readline()
    rest = rfile.read()
    rfile.close()
    conn.close()
    print("makefile_bin:", line1, line2, line3, rest)

    # Text makefile with UTF-8 and universal newlines
    cli2 = socket.create_connection(addr, timeout=2.0)
    conn2, _ = srv.accept()
    tw = cli2.makefile("w", encoding="utf-8", newline="\n")
    tr = conn2.makefile("r", encoding="utf-8")
    tw.write("first line\nsecond line\n")
    tw.flush()
    tw.close()
    cli2.close()
    lines = tr.readlines()
    tr.close()
    conn2.close()
    print("makefile_text:", lines)

# 7. UDP loopback: bind/sendto/recvfrom & connected UDP send/recv
with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp_srv:
    udp_srv.bind(("127.0.0.1", 0))
    udp_addr = udp_srv.getsockname()
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp_cli:
        udp_cli.settimeout(2.0)
        udp_srv.settimeout(2.0)
        n = udp_cli.sendto(b"ping-udp", udp_addr)
        assert n == 8
        data, sender = udp_srv.recvfrom(1024)
        assert data == b"ping-udp"
        assert sender[0] == "127.0.0.1"
        udp_srv.sendto(b"pong-udp", sender)
        reply, from_addr = udp_cli.recvfrom(1024)
        assert reply == b"pong-udp"
        assert from_addr == udp_addr
print("udp_loopback_ok")
