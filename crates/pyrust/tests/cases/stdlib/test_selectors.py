import selectors
import socket

# 1. Constants, SelectorKey, inheritance
assert selectors.EVENT_READ == 1
assert selectors.EVENT_WRITE == 2
assert selectors.SelectorKey._fields == ("fileobj", "fd", "events", "data")
assert issubclass(selectors.SelectSelector, selectors.BaseSelector)
assert issubclass(selectors.DefaultSelector, selectors.BaseSelector)

k = selectors.SelectorKey("obj", 5, selectors.EVENT_READ, {"tag": "test"})
assert (k.fileobj, k.fd, k.events, k.data) == ("obj", 5, 1, {"tag": "test"})
print("selectors_basics_ok")

# 2. Register, get_key, get_map, modify, unregister, error handling, context manager
with socket.create_server(("127.0.0.1", 0)) as srv:
    addr = srv.getsockname()
    with selectors.DefaultSelector() as sel:
        key = sel.register(srv, selectors.EVENT_READ, data="listener")
        assert key.fileobj is srv
        assert key.fd == srv.fileno()
        assert key.events == selectors.EVENT_READ
        assert key.data == "listener"

        assert sel.get_key(srv) == key
        mapping = sel.get_map()
        assert len(mapping) == 1
        assert srv in mapping
        assert mapping[srv] == key
        assert mapping.get(srv) == key
        assert list(mapping) == [srv.fileno()]
        assert srv.fileno() in mapping

        # Duplicate registration raises KeyError
        try:
            sel.register(srv, selectors.EVENT_READ)
            assert False
        except KeyError:
            pass

        # Invalid event mask raises ValueError
        try:
            sel.register(srv, 0)
            assert False
        except ValueError:
            pass

        # No pending connection -> timeout returns empty list
        ready0 = sel.select(timeout=0.02)
        assert ready0 == []

        # Connect client -> listening socket becomes readable
        cli = socket.create_connection(addr, timeout=2.0)
        try:
            ready1 = sel.select(timeout=1.0)
            assert len(ready1) == 1
            rkey, revents = ready1[0]
            assert rkey.fileobj is srv
            assert rkey.data == "listener"
            assert (revents & selectors.EVENT_READ) != 0

            conn, _ = srv.accept()
            try:
                # Register connected socket for write -> immediately writable
                ckey = sel.register(conn, selectors.EVENT_WRITE, data="conn_w")
                ready_w = sel.select(timeout=1.0)
                assert any(
                    rk.fileobj is conn and (ev & selectors.EVENT_WRITE) != 0
                    for rk, ev in ready_w
                )

                # Modify to watch EVENT_READ with updated data
                ckey2 = sel.modify(conn, selectors.EVENT_READ, data="conn_r")
                assert ckey2.events == selectors.EVENT_READ
                assert ckey2.data == "conn_r"
                assert sel.get_key(conn).data == "conn_r"

                # Not readable before client sends data
                ready_idle = sel.select(timeout=0.02)
                assert ready_idle == []

                # Client sends data -> conn becomes readable
                cli.sendall(b"selector-payload")
                ready_r = sel.select(timeout=1.0)
                assert len(ready_r) == 1
                rk, ev = ready_r[0]
                assert rk.fileobj is conn
                assert rk.data == "conn_r"
                assert (ev & selectors.EVENT_READ) != 0
                payload = conn.recv(64)
                print("selector_tcp_payload:", payload)

                # Modify only data (events unchanged)
                ckey3 = sel.modify(conn, selectors.EVENT_READ, data="conn_r2")
                assert ckey3.data == "conn_r2"

                # Unregister conn
                unreg = sel.unregister(conn)
                assert unreg.fileobj is conn
                assert len(sel.get_map()) == 1

                try:
                    sel.get_key(conn)
                    assert False
                except KeyError:
                    pass
            finally:
                conn.close()
        finally:
            cli.close()

    # After context exit, selector is closed
    assert sel.get_map() is None
    try:
        sel.get_key(srv)
        assert False
    except RuntimeError:
        pass
print("selectors_tcp_ok")

# 3. Explicit SelectSelector with UDP loopback socket
with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp_srv:
    udp_srv.bind(("127.0.0.1", 0))
    udp_addr = udp_srv.getsockname()
    with selectors.SelectSelector() as sel:
        sel.register(udp_srv, selectors.EVENT_READ, data="udp")
        assert sel.select(timeout=0.02) == []
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp_cli:
            udp_cli.sendto(b"sel-udp", udp_addr)
            ready = sel.select(timeout=1.0)
            assert len(ready) == 1
            rk, ev = ready[0]
            assert rk.fileobj is udp_srv and rk.data == "udp"
            assert (ev & selectors.EVENT_READ) != 0
            msg, _ = udp_srv.recvfrom(64)
            print("selector_udp_payload:", msg)
print("selectors_udp_ok")
