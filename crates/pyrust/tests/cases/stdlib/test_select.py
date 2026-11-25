import select
import socket
import sys

# 1. Module metadata and error alias
assert select.error is OSError
print("select_error_ok")

# 2. Validation errors
try:
    select.select([-1], [], [], 0)
    assert False, "expected ValueError for negative fd"
except ValueError:
    pass

try:
    select.select(["not-a-fd"], [], [], 0)
    assert False, "expected TypeError for non-fd object"
except TypeError:
    pass

with socket.create_server(("127.0.0.1", 0)) as s_bad_timeout:
    try:
        select.select([s_bad_timeout], [], [], -1.0)
        assert False, "expected ValueError or OSError for negative timeout"
    except (ValueError, OSError):
        pass
print("select_validation_ok")

# 3. TCP loopback readiness with select.select()
with socket.create_server(("127.0.0.1", 0)) as srv:
    addr = srv.getsockname()

    # No pending client -> empty readiness lists on short timeout
    r, w, x = select.select([srv], [], [], 0.02)
    assert r == [] and w == [] and x == []

    cli = socket.create_connection(addr, timeout=2.0)
    try:
        # Listener becomes readable once client connects
        r, w, x = select.select([srv], [], [], 1.0)
        assert r == [srv] and w == [] and x == []

        conn, _ = srv.accept()
        try:
            # Connected socket is immediately writable, not yet readable
            r, w, x = select.select([conn], [conn], [conn], 1.0)
            assert r == [] and w == [conn] and x == []

            # Custom wrapper with .fileno() is returned verbatim
            class FdWrapper:
                def __init__(self, sock):
                    self._sock = sock

                def fileno(self):
                    return self._sock.fileno()

            wrapped = FdWrapper(conn)
            r, w, x = select.select([wrapped], [], [], 0.02)
            assert r == []

            cli.sendall(b"select-tcp-hello")
            r, w, x = select.select([wrapped], [], [], 1.0)
            assert r == [wrapped] and w == [] and x == []

            # Raw integer fileno also works
            r_int, _, _ = select.select([conn.fileno()], [], [], 1.0)
            assert r_int == [conn.fileno()]

            data = conn.recv(64)
            print("select_tcp_payload:", data)
        finally:
            conn.close()
    finally:
        cli.close()
print("select_tcp_ok")

# 4. UDP loopback readiness with select.select()
with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp_srv:
    udp_srv.bind(("127.0.0.1", 0))
    udp_addr = udp_srv.getsockname()
    r, w, x = select.select([udp_srv], [], [], 0.02)
    assert r == [] and w == [] and x == []
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp_cli:
        udp_cli.sendto(b"select-udp-ping", udp_addr)
        r, w, x = select.select([udp_srv], [], [], 1.0)
        assert r == [udp_srv]
        payload, _ = udp_srv.recvfrom(64)
        print("select_udp_payload:", payload)
print("select_udp_ok")

# 5. Unix select.poll() parity
if hasattr(select, "poll"):
    assert isinstance(select.POLLIN, int)
    assert isinstance(select.POLLPRI, int)
    assert isinstance(select.POLLOUT, int)
    assert isinstance(select.POLLERR, int)
    assert isinstance(select.POLLHUP, int)
    assert isinstance(select.POLLNVAL, int)

    with socket.create_server(("127.0.0.1", 0)) as srv:
        addr = srv.getsockname()
        p = select.poll()
        p.register(srv, select.POLLIN)
        assert p.poll(10) == []

        cli = socket.create_connection(addr, timeout=2.0)
        try:
            events = p.poll(1000)
            assert len(events) == 1
            ev_fd, ev_mask = events[0]
            assert ev_fd == srv.fileno()
            assert (ev_mask & select.POLLIN) != 0

            conn, _ = srv.accept()
            try:
                p.register(conn, select.POLLOUT)
                out_events = p.poll(1000)
                assert any(
                    fd == conn.fileno() and (m & select.POLLOUT) != 0
                    for fd, m in out_events
                )

                p.modify(conn, select.POLLIN)
                cli.sendall(b"poll-msg")
                in_events = p.poll(1000)
                assert any(
                    fd == conn.fileno() and (m & select.POLLIN) != 0
                    for fd, m in in_events
                )
                assert conn.recv(64) == b"poll-msg"

                p.unregister(conn)
                try:
                    p.unregister(conn)
                    assert False, "expected KeyError"
                except KeyError:
                    pass
                try:
                    p.modify(conn, select.POLLIN)
                    assert False, "expected OSError"
                except OSError:
                    pass
            finally:
                conn.close()
        finally:
            cli.close()
            p.unregister(srv)

# 6. Linux select.epoll() parity
if sys.platform.startswith("linux") and hasattr(select, "epoll"):
    assert isinstance(select.EPOLLIN, int)
    assert isinstance(select.EPOLLOUT, int)
    assert isinstance(select.EPOLLET, int)
    assert isinstance(select.EPOLLONESHOT, int)

    with socket.create_server(("127.0.0.1", 0)) as srv:
        addr = srv.getsockname()
        with select.epoll() as ep:
            assert ep.closed is False
            assert isinstance(ep.fileno(), int) and ep.fileno() >= 0
            ep.register(srv, select.EPOLLIN)
            assert ep.poll(0.01) == []
            cli = socket.create_connection(addr, timeout=2.0)
            try:
                evs = ep.poll(1.0)
                assert any(
                    fd == srv.fileno() and (m & select.EPOLLIN) != 0
                    for fd, m in evs
                )
                conn, _ = srv.accept()
                conn.close()
            finally:
                cli.close()
            ep.unregister(srv)
        assert ep.closed is True
        try:
            ep.fileno()
            assert False, "expected ValueError on closed epoll"
        except ValueError:
            pass

# 7. macOS select.kqueue() / select.kevent parity
if sys.platform == "darwin" and hasattr(select, "kqueue"):
    assert isinstance(select.KQ_FILTER_READ, int)
    assert isinstance(select.KQ_EV_ADD, int)
    with select.kqueue() as kq:
        assert kq.closed is False
        assert isinstance(kq.fileno(), int) and kq.fileno() >= 0
        with socket.create_server(("127.0.0.1", 0)) as srv:
            addr = srv.getsockname()
            kev = select.kevent(srv.fileno(), filter=select.KQ_FILTER_READ, flags=select.KQ_EV_ADD)
            assert kev.ident == srv.fileno()
            assert kq.control([kev], 1, 0.01) == []
            cli = socket.create_connection(addr, timeout=2.0)
            try:
                got = kq.control(None, 4, 1.0)
                assert len(got) >= 1 and got[0].ident == srv.fileno()
                conn, _ = srv.accept()
                conn.close()
            finally:
                cli.close()
    assert kq.closed is True

print("select_all_ok")
