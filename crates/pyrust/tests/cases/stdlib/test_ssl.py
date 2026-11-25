"""Comprehensive 100% offline CPython 3.12 parity tests for ssl, _ssl,
http.client.HTTPSConnection, and urllib.request.HTTPSHandler (issue #3252).
"""

import http.client
import os
import socket
import ssl
import _ssl
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request

# ---------------------------------------------------------------------------
# Hardcoded RFC 5280 ECDSA P-256 test CA + leaf certificate (valid until 2098)
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
# 1. Module identity, exceptions, enums, constants, PEM/DER helpers
# ---------------------------------------------------------------------------
assert ssl.SSLError is _ssl.SSLError
assert ssl.SSLZeroReturnError is _ssl.SSLZeroReturnError
assert ssl.SSLWantReadError is _ssl.SSLWantReadError
assert ssl.SSLWantWriteError is _ssl.SSLWantWriteError
assert ssl.SSLSyscallError is _ssl.SSLSyscallError
assert ssl.SSLEOFError is _ssl.SSLEOFError
assert ssl.SSLCertVerificationError is _ssl.SSLCertVerificationError
assert ssl.MemoryBIO is _ssl.MemoryBIO

assert issubclass(ssl.SSLError, OSError)
assert issubclass(ssl.SSLZeroReturnError, ssl.SSLError)
assert issubclass(ssl.SSLWantReadError, ssl.SSLError)
assert issubclass(ssl.SSLWantWriteError, ssl.SSLError)
assert issubclass(ssl.SSLSyscallError, ssl.SSLError)
assert issubclass(ssl.SSLEOFError, ssl.SSLError)
assert issubclass(ssl.SSLCertVerificationError, ssl.SSLError)
assert issubclass(ssl.SSLCertVerificationError, ValueError)

assert ssl.CERT_NONE == 0 and ssl.VerifyMode.CERT_NONE == 0
assert ssl.CERT_OPTIONAL == 1 and ssl.VerifyMode.CERT_OPTIONAL == 1
assert ssl.CERT_REQUIRED == 2 and ssl.VerifyMode.CERT_REQUIRED == 2
assert ssl.TLSVersion.MINIMUM_SUPPORTED == -2
assert ssl.TLSVersion.SSLv3 == 0x300
assert ssl.TLSVersion.TLSv1 == 0x301
assert ssl.TLSVersion.TLSv1_1 == 0x302
assert ssl.TLSVersion.TLSv1_2 == 0x303
assert ssl.TLSVersion.TLSv1_3 == 0x304
assert ssl.TLSVersion.MAXIMUM_SUPPORTED == -1
assert ssl.PROTOCOL_TLS_CLIENT == 16
assert ssl.PROTOCOL_TLS_SERVER == 17
assert ssl.PROTOCOL_TLS == 2
assert ssl.PROTOCOL_TLSv1_2 == 5
assert ssl.get_protocol_name(ssl.PROTOCOL_TLS_CLIENT) == "PROTOCOL_TLS_CLIENT"
assert ssl.get_protocol_name(ssl.PROTOCOL_TLS_SERVER) == "PROTOCOL_TLS_SERVER"
assert isinstance(ssl.Purpose.SERVER_AUTH, ssl.Purpose)
assert isinstance(ssl.Purpose.CLIENT_AUTH, ssl.Purpose)
assert ssl.Purpose.SERVER_AUTH != ssl.Purpose.CLIENT_AUTH
assert ssl.Purpose.SERVER_AUTH.oid == "1.3.6.1.5.5.7.3.1"
assert ssl.Purpose.CLIENT_AUTH.oid == "1.3.6.1.5.5.7.3.2"

assert ssl.HAS_SNI is True
assert ssl.HAS_ALPN is True
assert ssl.HAS_TLSv1_2 is True
assert ssl.HAS_TLSv1_3 is True
assert ssl.HAS_NPN is False
assert isinstance(ssl.OPENSSL_VERSION, str) and len(ssl.OPENSSL_VERSION) > 0
assert isinstance(ssl.OPENSSL_VERSION_INFO, tuple) and len(ssl.OPENSSL_VERSION_INFO) == 5
assert isinstance(ssl.OPENSSL_VERSION_NUMBER, int) and ssl.OPENSSL_VERSION_NUMBER > 0

# DefaultVerifyPaths structseq
dvp = ssl.get_default_verify_paths()
assert isinstance(dvp, tuple) and len(dvp) == 6
assert dvp.openssl_cafile_env == "SSL_CERT_FILE"
assert dvp.openssl_capath_env == "SSL_CERT_DIR"
assert isinstance(dvp.openssl_cafile, str)
assert isinstance(dvp.openssl_capath, str)

# PEM <-> DER roundtrip
srv_der = ssl.PEM_cert_to_DER_cert(TEST_SERVER_CERT_PEM)
assert isinstance(srv_der, bytes) and len(srv_der) > 100
srv_pem_rt = ssl.DER_cert_to_PEM_cert(srv_der)
assert srv_pem_rt == TEST_SERVER_CERT_PEM
assert ssl.PEM_cert_to_DER_cert(srv_pem_rt) == srv_der

try:
    ssl.PEM_cert_to_DER_cert("not a pem certificate")
    assert False, "expected ValueError"
except ValueError:
    pass

print("constants_and_pem_ok")

# ---------------------------------------------------------------------------
# 2. MemoryBIO behavior
# ---------------------------------------------------------------------------
bio = ssl.MemoryBIO()
assert bio.pending == 0
assert bio.eof is False
assert bio.read() == b""
assert bio.write(b"hello ") == 6
assert bio.write(b"world") == 5
assert bio.pending == 11
assert bio.read(5) == b"hello"
assert bio.pending == 6
bio.write_eof()
assert bio.eof is False  # still 6 bytes pending
try:
    bio.write(b"more")
    assert False, "expected SSLError after write_eof"
except ssl.SSLError:
    pass
assert bio.read() == b" world"
assert bio.pending == 0
assert bio.eof is True

print("memory_bio_ok")

# ---------------------------------------------------------------------------
# 3. SSLContext properties & configuration
# ---------------------------------------------------------------------------
c_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
assert c_ctx.protocol == ssl.PROTOCOL_TLS_CLIENT
assert c_ctx.verify_mode == ssl.CERT_REQUIRED
assert c_ctx.check_hostname is True
assert c_ctx.minimum_version == ssl.TLSVersion.TLSv1_2
assert c_ctx.maximum_version == ssl.TLSVersion.MAXIMUM_SUPPORTED

# Cannot set CERT_NONE while check_hostname is enabled
try:
    c_ctx.verify_mode = ssl.CERT_NONE
    assert False, "expected ValueError"
except ValueError:
    pass

c_ctx.check_hostname = False
c_ctx.verify_mode = ssl.CERT_NONE
assert c_ctx.verify_mode == ssl.CERT_NONE
assert c_ctx.check_hostname is False

# Re-enabling check_hostname forces CERT_REQUIRED
c_ctx.check_hostname = True
assert c_ctx.verify_mode == ssl.CERT_REQUIRED
assert c_ctx.check_hostname is True

# create_default_context / _create_unverified_context
def_ctx = ssl.create_default_context()
assert def_ctx.verify_mode == ssl.CERT_REQUIRED
assert def_ctx.check_hostname is True
stats = def_ctx.cert_store_stats()
assert stats["x509"] > 0 and stats["x509_ca"] > 0 and stats["crl"] == 0

ca_ctx = ssl.create_default_context(cadata=TEST_CA_CERT_PEM)
assert ca_ctx.verify_mode == ssl.CERT_REQUIRED
assert ca_ctx.check_hostname is True
ca_certs_der = ca_ctx.get_ca_certs(binary_form=True)
assert len(ca_certs_der) == 1
assert ca_certs_der[0] == ssl.PEM_cert_to_DER_cert(TEST_CA_CERT_PEM)
ca_certs_parsed = ca_ctx.get_ca_certs(binary_form=False)
assert len(ca_certs_parsed) == 1
assert ca_certs_parsed[0]["version"] == 3

unver_ctx = ssl._create_unverified_context()
assert unver_ctx.verify_mode == ssl.CERT_NONE
assert unver_ctx.check_hostname is False

print("ssl_context_ok")

# ---------------------------------------------------------------------------
# 4. In-memory TLS 1.3 and TLS 1.2 handshakes & I/O via MemoryBIO + wrap_bio
# ---------------------------------------------------------------------------
def _pump_bio(src_out, dst_in):
    data = src_out.read()
    if data:
        dst_in.write(data)
        return True
    return False


def _run_bio_handshake(client_obj, c_in, c_out, server_obj, s_in, s_out):
    c_done = False
    s_done = False
    for _ in range(32):
        if not c_done:
            try:
                client_obj.do_handshake()
                c_done = True
            except ssl.SSLWantReadError:
                pass
        _pump_bio(c_out, s_in)
        if not s_done:
            try:
                server_obj.do_handshake()
                s_done = True
            except ssl.SSLWantReadError:
                pass
        _pump_bio(s_out, c_in)
        if c_done and s_done:
            return
    raise AssertionError("TLS BIO handshake did not converge")


with tempfile.TemporaryDirectory() as tmpdir:
    ca_file = os.path.join(tmpdir, "ca.pem")
    cert_file = os.path.join(tmpdir, "srv.pem")
    key_file = os.path.join(tmpdir, "srv.key")
    with open(ca_file, "w", encoding="ascii") as f:
        f.write(TEST_CA_CERT_PEM)
    with open(cert_file, "w", encoding="ascii") as f:
        f.write(TEST_SERVER_CERT_PEM)
    with open(key_file, "w", encoding="ascii") as f:
        f.write(TEST_SERVER_KEY_PEM)

    # 4a. TLS 1.3 verified handshake with SNI + ALPN + cert inspection + data exchange
    srv_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    srv_ctx.load_cert_chain(cert_file, key_file)
    srv_ctx.set_alpn_protocols(["h2", "http/1.1"])

    cli_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    cli_ctx.load_verify_locations(cafile=ca_file)
    cli_ctx.set_alpn_protocols(["h2", "http/1.1"])

    c_in, c_out = ssl.MemoryBIO(), ssl.MemoryBIO()
    s_in, s_out = ssl.MemoryBIO(), ssl.MemoryBIO()
    cli_obj = cli_ctx.wrap_bio(c_in, c_out, server_side=False, server_hostname="localhost")
    srv_obj = srv_ctx.wrap_bio(s_in, s_out, server_side=True)

    assert cli_obj.server_hostname == "localhost"
    assert cli_obj.server_side is False
    assert cli_obj.context is cli_ctx
    assert srv_obj.server_side is True
    assert srv_obj.server_hostname is None

    _run_bio_handshake(cli_obj, c_in, c_out, srv_obj, s_in, s_out)

    assert cli_obj.version() == "TLSv1.3"
    assert srv_obj.version() == "TLSv1.3"
    assert cli_obj.selected_alpn_protocol() == "h2"
    assert srv_obj.selected_alpn_protocol() == "h2"
    c_cipher = cli_obj.cipher()
    assert isinstance(c_cipher, tuple) and len(c_cipher) == 3
    assert c_cipher[1] == "TLSv1.3" and c_cipher[2] >= 128

    # Peer certificate binary and parsed forms
    peer_der = cli_obj.getpeercert(binary_form=True)
    assert peer_der == srv_der
    peer_dict = cli_obj.getpeercert(binary_form=False)
    assert isinstance(peer_dict, dict)
    assert peer_dict["version"] == 3
    assert (("commonName", "localhost"),) in peer_dict["subject"]
    assert (("commonName", "PyRust Test CA"),) in peer_dict["issuer"]
    assert ("DNS", "localhost") in peer_dict["subjectAltName"]
    assert ("IP Address", "127.0.0.1") in peer_dict["subjectAltName"]
    assert peer_dict["notBefore"].endswith("GMT")
    assert peer_dict["notAfter"].endswith("GMT")
    assert len(peer_dict["serialNumber"]) > 0

    # Bidirectional encrypted data exchange (including buffer read)
    assert cli_obj.write(b"ping over TLS 1.3") == 17
    _pump_bio(c_out, s_in)
    assert srv_obj.read(64) == b"ping over TLS 1.3"

    assert srv_obj.write(b"pong over TLS 1.3") == 17
    _pump_bio(s_out, c_in)
    buf = bytearray(32)
    nread = cli_obj.read(32, buf)
    assert nread == 17 and bytes(buf[:nread]) == b"pong over TLS 1.3"

    # Clean unwrap / close_notify
    try:
        cli_obj.unwrap()
    except ssl.SSLWantReadError:
        pass
    _pump_bio(c_out, s_in)
    assert srv_obj.read(16) == b""

    print("bio_tls13_ok")

    # 4b. TLS 1.2 forced handshake via minimum_version / maximum_version
    srv_ctx12 = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    srv_ctx12.load_cert_chain(cert_file, key_file)
    srv_ctx12.minimum_version = ssl.TLSVersion.TLSv1_2
    srv_ctx12.maximum_version = ssl.TLSVersion.TLSv1_2

    cli_ctx12 = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    cli_ctx12.load_verify_locations(cadata=TEST_CA_CERT_PEM)
    cli_ctx12.minimum_version = ssl.TLSVersion.TLSv1_2
    cli_ctx12.maximum_version = ssl.TLSVersion.TLSv1_2

    c_in2, c_out2 = ssl.MemoryBIO(), ssl.MemoryBIO()
    s_in2, s_out2 = ssl.MemoryBIO(), ssl.MemoryBIO()
    cli_obj12 = cli_ctx12.wrap_bio(c_in2, c_out2, server_hostname="127.0.0.1")
    srv_obj12 = srv_ctx12.wrap_bio(s_in2, s_out2, server_side=True)
    _run_bio_handshake(cli_obj12, c_in2, c_out2, srv_obj12, s_in2, s_out2)
    assert cli_obj12.version() == "TLSv1.2"
    assert srv_obj12.version() == "TLSv1.2"
    assert cli_obj12.write(b"tls12-ok") == 8
    _pump_bio(c_out2, s_in2)
    assert srv_obj12.read(32) == b"tls12-ok"

    print("bio_tls12_ok")

    # 4c. Untrusted certificate raises SSLCertVerificationError
    untrusted_cli_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    c_in3, c_out3 = ssl.MemoryBIO(), ssl.MemoryBIO()
    s_in3, s_out3 = ssl.MemoryBIO(), ssl.MemoryBIO()
    cli_bad = untrusted_cli_ctx.wrap_bio(c_in3, c_out3, server_hostname="localhost")
    srv_ok = srv_ctx.wrap_bio(s_in3, s_out3, server_side=True)
    try:
        _run_bio_handshake(cli_bad, c_in3, c_out3, srv_ok, s_in3, s_out3)
        assert False, "expected SSLCertVerificationError for untrusted CA"
    except ssl.SSLCertVerificationError as err:
        assert isinstance(err, ssl.SSLError)
        assert isinstance(err, ValueError)
        assert err.verify_code != 0
        assert isinstance(err.verify_message, str) and len(err.verify_message) > 0

    # 4d. Hostname mismatch raises SSLCertVerificationError
    c_in4, c_out4 = ssl.MemoryBIO(), ssl.MemoryBIO()
    s_in4, s_out4 = ssl.MemoryBIO(), ssl.MemoryBIO()
    cli_mismatch = cli_ctx.wrap_bio(c_in4, c_out4, server_hostname="wrong.example.com")
    srv_ok2 = srv_ctx.wrap_bio(s_in4, s_out4, server_side=True)
    try:
        _run_bio_handshake(cli_mismatch, c_in4, c_out4, srv_ok2, s_in4, s_out4)
        assert False, "expected SSLCertVerificationError for hostname mismatch"
    except ssl.SSLCertVerificationError as err:
        assert err.verify_code != 0

    # 4e. CERT_REQUIRED with check_hostname=False verifies chain but ignores hostname mismatch
    chain_only_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    chain_only_ctx.check_hostname = False
    chain_only_ctx.verify_mode = ssl.CERT_REQUIRED
    chain_only_ctx.load_verify_locations(cafile=ca_file, capath=tmpdir)
    c_in4b, c_out4b = ssl.MemoryBIO(), ssl.MemoryBIO()
    s_in4b, s_out4b = ssl.MemoryBIO(), ssl.MemoryBIO()
    cli_chain_only = chain_only_ctx.wrap_bio(c_in4b, c_out4b, server_hostname="wrong.example.com")
    srv_ok2b = srv_ctx.wrap_bio(s_in4b, s_out4b, server_side=True)
    _run_bio_handshake(cli_chain_only, c_in4b, c_out4b, srv_ok2b, s_in4b, s_out4b)
    assert (("commonName", "localhost"),) in cli_chain_only.getpeercert()["subject"]

    # 4f. CERT_NONE skips verification & getpeercert(False) returns {}
    nover_ctx = ssl._create_unverified_context()
    c_in5, c_out5 = ssl.MemoryBIO(), ssl.MemoryBIO()
    s_in5, s_out5 = ssl.MemoryBIO(), ssl.MemoryBIO()
    cli_nover = nover_ctx.wrap_bio(c_in5, c_out5, server_hostname="wrong.example.com")
    srv_ok3 = srv_ctx.wrap_bio(s_in5, s_out5, server_side=True)
    _run_bio_handshake(cli_nover, c_in5, c_out5, srv_ok3, s_in5, s_out5)
    assert cli_nover.getpeercert(binary_form=False) == {}
    assert cli_nover.getpeercert(binary_form=True) == srv_der

    # 4g. check_hostname=True without server_hostname raises ValueError on wrap_socket
    tmp_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        try:
            cli_ctx.wrap_socket(tmp_sock)
            assert False, "expected ValueError when server_hostname is missing"
        except ValueError:
            pass
    finally:
        tmp_sock.close()

    # 4h. set_ciphers accepts valid cipher string and rejects empty string
    cli_ctx.set_ciphers("DEFAULT")
    try:
        cli_ctx.set_ciphers("")
        assert False, "expected SSLError for empty cipher list"
    except ssl.SSLError:
        pass

    print("bio_verification_ok")

    # -----------------------------------------------------------------------
    # 5. Loopback HTTPS server in subprocess:
    #    - raw SSLSocket client & server accept()
    #    - http.client.HTTPSConnection (GET & POST)
    #    - urllib.request.urlopen(..., context=...) & HTTPSHandler
    # -----------------------------------------------------------------------
    assert hasattr(http.client, "HTTPSConnection")
    assert "HTTPSConnection" in http.client.__all__
    assert hasattr(urllib.request, "HTTPSHandler")
    assert "HTTPSHandler" in urllib.request.__all__

    _HTTPS_SERVER_CODE = r'''
import http.server
import ssl
import sys

cert_file = sys.argv[1]
key_file = sys.argv[2]

class HTTPSHandlerImpl(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):
        pass

    def _send(self, code, body, ctype="text/plain"):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        if body:
            self.wfile.write(body)

    def do_GET(self):
        if self.path == "/__shutdown":
            self._send(200, b"tls-bye")
            self.server._done = True
            return
        if self.path == "/hello":
            hdr = self.headers.get("X-Client", "")
            self._send(200, f"HTTPS-GET:{hdr}".encode("utf-8"))
            return
        self._send(404, b"not-found")

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length) if length > 0 else b""
        self._send(201, b"HTTPS-POST:" + body)

srv_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
srv_ctx.load_cert_chain(cert_file, key_file)
srv_ctx.set_alpn_protocols(["http/1.1"])

with http.server.HTTPServer(("127.0.0.1", 0), HTTPSHandlerImpl) as srv:
    srv.socket = srv_ctx.wrap_socket(srv.socket, server_side=True)
    srv._done = False
    port = srv.server_address[1]
    sys.stdout.write(f"READY {port}\n")
    sys.stdout.flush()
    while not srv._done:
        try:
            srv.handle_request()
        except ssl.SSLError:
            pass
'''

    proc = subprocess.Popen(
        [sys.executable, "-c", _HTTPS_SERVER_CODE, cert_file, key_file],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        ready_line = proc.stdout.readline().decode("ascii").strip()
        assert ready_line.startswith("READY "), f"unexpected server line: {ready_line!r}"
        port = int(ready_line.split()[1])

        # 5a. http.client.HTTPSConnection GET & POST with verified context
        https_ctx = ssl.create_default_context(cafile=ca_file)
        https_ctx.set_alpn_protocols(["http/1.1"])

        conn = http.client.HTTPSConnection("127.0.0.1", port, context=https_ctx, timeout=5)
        try:
            conn.request("GET", "/hello", headers={"X-Client": "hc"})
            resp = conn.getresponse()
            assert resp.status == 200
            assert resp.read() == b"HTTPS-GET:hc"
            assert conn.sock is None or isinstance(conn.sock, ssl.SSLSocket)
        finally:
            conn.close()

        conn2 = http.client.HTTPSConnection("127.0.0.1", port, context=https_ctx, timeout=5)
        try:
            conn2.request("POST", "/echo", body=b"secret-payload")
            resp2 = conn2.getresponse()
            assert resp2.status == 201
            assert resp2.read() == b"HTTPS-POST:secret-payload"
        finally:
            conn2.close()

        print("https_connection_ok")

        # 5b. Raw SSLSocket connect / sendall / recv / getpeercert / alpn
        raw = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        raw.settimeout(5)
        ssock = https_ctx.wrap_socket(raw, server_hostname="localhost")
        assert isinstance(ssock, socket.socket)
        assert isinstance(ssock, ssl.SSLSocket)
        with ssock:
            ssock.connect(("127.0.0.1", port))
            assert ssock.version() in ("TLSv1.3", "TLSv1.2")
            assert ssock.selected_alpn_protocol() == "http/1.1"
            assert ssock.getpeercert(binary_form=True) == srv_der
            assert (("commonName", "localhost"),) in ssock.getpeercert()["subject"]
            ssock.sendall(
                b"GET /hello HTTP/1.1\r\nHost: localhost\r\nX-Client: raw\r\nConnection: close\r\n\r\n"
            )
            chunks = []
            while True:
                data = ssock.recv(4096)
                if not data:
                    break
                chunks.append(data)
            raw_resp = b"".join(chunks)
            assert b"HTTP/1.1 200 OK" in raw_resp
            assert raw_resp.endswith(b"HTTPS-GET:raw")

        print("raw_sslsocket_ok")

        # 5c. urllib.request.urlopen(..., context=...) GET & POST
        url = f"https://127.0.0.1:{port}"
        req = urllib.request.Request(f"{url}/hello", headers={"X-Client": "urllib"})
        with urllib.request.urlopen(req, context=https_ctx, timeout=5) as resp:
            assert resp.status == 200
            assert resp.read() == b"HTTPS-GET:urllib"

        with urllib.request.urlopen(
            f"{url}/submit", data=b"tls-form-body", context=https_ctx, timeout=5
        ) as resp:
            assert resp.status == 201
            assert resp.read() == b"HTTPS-POST:tls-form-body"

        # 5d. urllib.request.HTTPSHandler via build_opener
        opener = urllib.request.build_opener(urllib.request.HTTPSHandler(context=https_ctx))
        with opener.open(f"{url}/hello", timeout=5) as resp:
            assert resp.status == 200
            assert resp.read() == b"HTTPS-GET:"

        # 5e. Untrusted context fails via urllib.request.urlopen with URLError(SSLCertVerificationError)
        bad_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        try:
            urllib.request.urlopen(f"{url}/hello", context=bad_ctx, timeout=5)
            assert False, "expected URLError for untrusted HTTPS server"
        except urllib.error.URLError as err:
            assert isinstance(err.reason, ssl.SSLCertVerificationError)

        # 5f. Clean shutdown of HTTPS server
        with urllib.request.urlopen(f"{url}/__shutdown", context=https_ctx, timeout=5) as resp:
            assert resp.status == 200
            assert resp.read() == b"tls-bye"
    finally:
        try:
            proc.wait(timeout=10)
        except Exception:
            proc.kill()
            proc.wait(timeout=5)
        if proc.stdout:
            proc.stdout.close()
        if proc.stderr:
            proc.stderr.close()

    assert proc.returncode == 0
    print("https_urllib_loopback_ok")

print("ssl_all_ok")
