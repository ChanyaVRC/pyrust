"""Offline deterministic CPython 3.12 parity tests for asyncio streams,
transports, protocols, TLS streams, UDP datagrams, and low-level socket APIs
(issue #3254).
"""

import asyncio
import os
import socket
import ssl
import tempfile

# ---------------------------------------------------------------------------
# Offline RFC 5280 ECDSA P-256 test CA + localhost server cert (valid to 2098)
# SANs: DNS:localhost, IP:127.0.0.1
# ---------------------------------------------------------------------------
TEST_CA_CERT_PEM = """-----BEGIN CERTIFICATE-----
MIIBuzCCAWGgAwIBAgIUR0pB+4yV55iF7GoiW28y74jE/nYwCgYIKoZIzj0EAwIw
KjEXMBUGA1UEAwwOUHlSdXN0IFRlc3QgQ0ExDzANBgNVBAoMBlB5UnVzdDAgFw0y
NjExMjUyMjExNTVaGA8yMDk4MDEzMTIyMTE1NVowKjEXMBUGA1UEAwwOUHlSdXN0
IFRlc3QgQ0ExDzANBgNVBAoMBlB5UnVzdDBZMBMGByqGSM49AgEGCCqGSM49AwEH
A0IABMtGRVH2hmDzPLjAmAuJNAyuTkL8quhFLAJPgsMIAX5sRuwmHY5/P/0vaRVW
EaZzFZLiRh5dkvGkIVZ9X/k2A0ejYzBhMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0P
AQH/BAQDAgEGMB0GA1UdDgQWBBSKEVK7ygRMD4UPTvuki1WCxnfnTzAfBgNVHSME
GDAWgBSKEVK7ygRMD4UPTvuki1WCxnfnTzAKBggqhkjOPQQDAgNIADBFAiB9MK8a
T0+E827Lx/nheX16TfKUPondTQOInY9SqAzSTgIhAIDwgZ9+rlIO9W7LVr9JS+nn
8ojKrg8V67ml9ZWXbBSi
-----END CERTIFICATE-----
"""

TEST_SERVER_CERT_PEM = """-----BEGIN CERTIFICATE-----
MIIB/DCCAaKgAwIBAgIUcuEHd4Ljar/irUvyBlvrpZyMqfIwCgYIKoZIzj0EAwIw
KjEXMBUGA1UEAwwOUHlSdXN0IFRlc3QgQ0ExDzANBgNVBAoMBlB5UnVzdDAgFw0y
NjExMjUyMjExNTVaGA8yMDk4MDEzMTIyMTE1NVowMTESMBAGA1UEAwwJbG9jYWxo
b3N0MRswGQYDVQQKDBJQeVJ1c3QgVGVzdCBTZXJ2ZXIwWTATBgcqhkjOPQIBBggq
hkjOPQMBBwNCAAQxmf7ESOfIlo+nRtvX567US2IX+qLAxk/45fijzZZ8+A8KSboo
fxguGkq2Shhv4/BCu1k4NCnyriTEi4SzGBmso4GcMIGZMAwGA1UdEwEB/wQCMAAw
DgYDVR0PAQH/BAQDAgeAMB0GA1UdJQQWMBQGCCsGAQUFBwMBBggrBgEFBQcDAjAa
BgNVHREEEzARgglsb2NhbGhvc3SHBH8AAAEwHQYDVR0OBBYEFD+udiC6WbAeNEXm
/KUD74K5Jce5MB8GA1UdIwQYMBaAFIoRUrvKBEwPhQ9O+6SLVYLGd+dPMAoGCCqG
SM49BAMCA0gAMEUCID3oip3ams1Z9BL1JeH2gwTp2kz0+ceevd4r1bAKtpnJAiEA
0F9sIQaFhT8p/O6qmRLwsjZzSW+mqnze5Hab2mUoA8I=
-----END CERTIFICATE-----
"""

TEST_SERVER_KEY_PEM = """-----BEGIN EC PRIVATE KEY-----
MHcCAQEEIF1ZvAHMX6GiJJ8rWXpLnY+YRpPx7fjVEkpt+HQENlzfoAoGCCqGSM49
AwEHoUQDQgAEMZn+xEjnyJaPp0bb1+eu1EtiF/qiwMZP+OX4o82WfPgPCkm6KH8Y
LhpKtkoYb+PwQrtZODQp8q4kxIuEsxgZrA==
-----END EC PRIVATE KEY-----
"""


# ---------------------------------------------------------------------------
# 1. StreamReader manual feed, read*, exceptions, and async iteration
# ---------------------------------------------------------------------------
async def test_stream_reader_unit():
    r = asyncio.StreamReader(limit=16)
    assert not r.at_eof()
    r.feed_data(b"hello\nworld\nfoo::bar")
    assert await r.readline() == b"hello\n"
    assert await r.readexactly(6) == b"world\n"
    assert await r.readuntil(b"::") == b"foo::"
    r.feed_eof()
    assert await r.read(-1) == b"bar"
    assert r.at_eof()
    assert await r.read(10) == b""

    # IncompleteReadError on readexactly
    r2 = asyncio.StreamReader()
    r2.feed_data(b"abc")
    r2.feed_eof()
    try:
        await r2.readexactly(5)
        assert False, "expected IncompleteReadError"
    except asyncio.IncompleteReadError as e:
        assert e.partial == b"abc"
        assert e.expected == 5
        assert "3 bytes read on a total of 5 expected bytes" in str(e)

    # IncompleteReadError on readuntil when separator missing at EOF
    r3 = asyncio.StreamReader()
    r3.feed_data(b"partial")
    r3.feed_eof()
    try:
        await r3.readuntil(b"\r\n")
        assert False, "expected IncompleteReadError"
    except asyncio.IncompleteReadError as e:
        assert e.partial == b"partial"
        assert e.expected is None

    # LimitOverrunError on readuntil
    r4 = asyncio.StreamReader(limit=4)
    r4.feed_data(b"12345678\n")
    try:
        await r4.readuntil(b"\n")
        assert False, "expected LimitOverrunError"
    except asyncio.LimitOverrunError as e:
        assert e.consumed == 8

    # set_exception / exception
    r5 = asyncio.StreamReader()
    assert r5.exception() is None
    err = RuntimeError("stream failed")
    r5.set_exception(err)
    assert r5.exception() is err
    try:
        await r5.read(1)
        assert False, "expected RuntimeError"
    except RuntimeError as e:
        assert str(e) == "stream failed"

    # async for line in StreamReader
    r6 = asyncio.StreamReader()
    r6.feed_data(b"line1\nline2\nlast")
    r6.feed_eof()
    lines = [line async for line in r6]
    assert lines == [b"line1\n", b"line2\n", b"last"]
    print("stream_reader_unit: ok")


# ---------------------------------------------------------------------------
# 2. Loopback TCP start_server + open_connection (concurrent clients)
# ---------------------------------------------------------------------------
async def test_tcp_streams():
    async def handle_echo(reader, writer):
        data = await reader.readuntil(b"\n")
        writer.writelines([b"ECHO:", data])
        await writer.drain()
        if writer.can_write_eof():
            writer.write_eof()
        writer.close()
        await writer.wait_closed()

    server = await asyncio.start_server(handle_echo, "127.0.0.1", 0)
    assert server.is_serving()
    assert server.get_loop() is asyncio.get_running_loop()
    assert len(server.sockets) >= 1
    port = server.sockets[0].getsockname()[1]
    assert port > 0

    async def client(msg):
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        assert writer.transport is not None
        assert writer.get_extra_info("peername")[1] == port
        assert writer.get_extra_info("sockname")[0] == "127.0.0.1"
        assert writer.get_extra_info("missing_key", "fallback") == "fallback"
        writer.write(msg + b"\n")
        await writer.drain()
        reply = await reader.read(-1)
        writer.close()
        assert writer.is_closing()
        await writer.wait_closed()
        return reply

    async with server:
        replies = await asyncio.gather(
            client(b"alpha"),
            client(b"beta"),
            client(b"gamma"),
        )
    assert replies == [b"ECHO:alpha\n", b"ECHO:beta\n", b"ECHO:gamma\n"]
    assert not server.is_serving()
    print("tcp_streams: ok")


# ---------------------------------------------------------------------------
# 3. Offline Loopback TLS start_server + open_connection
# ---------------------------------------------------------------------------
async def test_tls_streams():
    with tempfile.TemporaryDirectory() as tmpdir:
        ca_path = os.path.join(tmpdir, "ca.pem")
        cert_path = os.path.join(tmpdir, "srv.pem")
        key_path = os.path.join(tmpdir, "srv.key")
        with open(ca_path, "w", encoding="utf-8") as f:
            f.write(TEST_CA_CERT_PEM)
        with open(cert_path, "w", encoding="utf-8") as f:
            f.write(TEST_SERVER_CERT_PEM)
        with open(key_path, "w", encoding="utf-8") as f:
            f.write(TEST_SERVER_KEY_PEM)

        server_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        server_ctx.load_cert_chain(certfile=cert_path, keyfile=key_path)

        client_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        client_ctx.load_verify_locations(cafile=ca_path)

        async def handle_tls(reader, writer):
            data = await reader.readline()
            writer.write(b"TLS:" + data)
            await writer.drain()
            writer.close()
            await writer.wait_closed()

        server = await asyncio.start_server(
            handle_tls, "127.0.0.1", 0, ssl=server_ctx
        )
        port = server.sockets[0].getsockname()[1]

        async with server:
            reader, writer = await asyncio.open_connection(
                "127.0.0.1", port, ssl=client_ctx, server_hostname="localhost"
            )
            ssl_obj = writer.get_extra_info("ssl_object")
            assert ssl_obj is not None
            assert writer.get_extra_info("sslcontext") is client_ctx
            assert writer.get_extra_info("cipher") is not None
            assert not writer.can_write_eof()

            writer.write(b"secret-hello\n")
            await writer.drain()
            resp = await reader.readline()
            assert resp == b"TLS:secret-hello\n"
            writer.close()
            await writer.wait_closed()
    print("tls_streams: ok")


# ---------------------------------------------------------------------------
# 4. Low-level create_server / create_connection with Protocol & BufferedProtocol
# ---------------------------------------------------------------------------
async def test_protocols_and_transports():
    loop = asyncio.get_running_loop()

    class UpperServerProtocol(asyncio.Protocol):
        def connection_made(self, transport):
            self.transport = transport

        def data_received(self, data):
            self.transport.write(data.upper())
            self.transport.close()

    class ClientProtocol(asyncio.Protocol):
        def __init__(self, on_done):
            self.on_done = on_done
            self.buf = bytearray()

        def connection_made(self, transport):
            transport.write(b"hello protocol")

        def data_received(self, data):
            self.buf.extend(data)

        def connection_lost(self, exc):
            if not self.on_done.done():
                self.on_done.set_result(bytes(self.buf))

    class ClientBufferedProtocol(asyncio.BufferedProtocol):
        def __init__(self, on_done):
            self.on_done = on_done
            self.buf = bytearray(64)
            self.received = bytearray()

        def connection_made(self, transport):
            transport.write(b"buffered")

        def get_buffer(self, sizehint):
            return self.buf

        def buffer_updated(self, nbytes):
            self.received.extend(self.buf[:nbytes])

        def connection_lost(self, exc):
            if not self.on_done.done():
                self.on_done.set_result(bytes(self.received))

    srv = await loop.create_server(UpperServerProtocol, "127.0.0.1", 0)
    port = srv.sockets[0].getsockname()[1]

    async with srv:
        done1 = loop.create_future()
        tr1, pr1 = await loop.create_connection(
            lambda: ClientProtocol(done1), "127.0.0.1", port
        )
        assert isinstance(tr1, asyncio.Transport)
        assert isinstance(pr1, asyncio.Protocol)
        assert await done1 == b"HELLO PROTOCOL"

        done2 = loop.create_future()
        tr2, pr2 = await loop.create_connection(
            lambda: ClientBufferedProtocol(done2), "127.0.0.1", port
        )
        assert isinstance(pr2, asyncio.BufferedProtocol)
        assert await done2 == b"BUFFERED"
    print("protocols_and_transports: ok")


# ---------------------------------------------------------------------------
# 5. UDP DatagramEndpoint + low-level sock_* APIs
# ---------------------------------------------------------------------------
async def test_datagram_and_sock_apis():
    loop = asyncio.get_running_loop()

    # UDP echo
    class UdpEcho(asyncio.DatagramProtocol):
        def connection_made(self, transport):
            self.transport = transport

        def datagram_received(self, data, addr):
            self.transport.sendto(b"UDP:" + data, addr)

    class UdpClient(asyncio.DatagramProtocol):
        def __init__(self, fut):
            self.fut = fut

        def connection_made(self, transport):
            transport.sendto(b"ping")

        def datagram_received(self, data, addr):
            if not self.fut.done():
                self.fut.set_result(data)

    srv_tr, _ = await loop.create_datagram_endpoint(
        UdpEcho, local_addr=("127.0.0.1", 0)
    )
    udp_port = srv_tr.get_extra_info("sockname")[1]
    fut = loop.create_future()
    cli_tr, _ = await loop.create_datagram_endpoint(
        lambda: UdpClient(fut), remote_addr=("127.0.0.1", udp_port)
    )
    assert isinstance(srv_tr, asyncio.DatagramTransport)
    assert await fut == b"UDP:ping"
    cli_tr.close()
    srv_tr.close()

    # Low-level sock_accept / sock_connect / sock_sendall / sock_recv / sock_recv_into
    lsock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    lsock.bind(("127.0.0.1", 0))
    lsock.listen(5)
    lsock.setblocking(False)
    sport = lsock.getsockname()[1]

    async def server_sock_task():
        conn, _addr = await loop.sock_accept(lsock)
        try:
            chunk = await loop.sock_recv(conn, 16)
            await loop.sock_sendall(conn, b"OK:" + chunk)
        finally:
            conn.close()

    st = asyncio.create_task(server_sock_task())
    csock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    csock.setblocking(False)
    try:
        await loop.sock_connect(csock, ("127.0.0.1", sport))
        await loop.sock_sendall(csock, b"rawsock")
        buf = bytearray(32)
        n = await loop.sock_recv_into(csock, buf)
        assert bytes(buf[:n]) == b"OK:rawsock"
    finally:
        csock.close()
        lsock.close()
    await st
    print("datagram_and_sock_apis: ok")


async def main():
    await test_stream_reader_unit()
    await test_tcp_streams()
    await test_tls_streams()
    await test_protocols_and_transports()
    await test_datagram_and_sock_apis()


asyncio.run(main())
