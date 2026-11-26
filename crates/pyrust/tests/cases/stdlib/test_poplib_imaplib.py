"""Comprehensive 100% offline deterministic CPython 3.12 parity tests for
poplib (POP3, POP3_SSL) and imaplib (IMAP4, IMAP4_SSL, helpers) (issue #3258).
"""

import datetime
import imaplib
import os
import poplib
import ssl
import subprocess
import sys
import tempfile
import time

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

# ===========================================================================
# 1. poplib & imaplib constants, exceptions, and helper functions
# ===========================================================================
assert poplib.POP3_PORT == 110
assert poplib.POP3_SSL_PORT == 995
assert poplib.CR == b"\r"
assert poplib.LF == b"\n"
assert poplib.CRLF == b"\r\n"
assert issubclass(poplib.error_proto, Exception)
assert issubclass(poplib.POP3_SSL, poplib.POP3)

assert imaplib.IMAP4_PORT == 143
assert imaplib.IMAP4_SSL_PORT == 993
assert imaplib.AllowedVersions == ("IMAP4REV1", "IMAP4")
assert "SELECT" in imaplib.Commands and "STARTTLS" in imaplib.Commands
assert issubclass(imaplib.IMAP4.error, Exception)
assert issubclass(imaplib.IMAP4.abort, imaplib.IMAP4.error)
assert issubclass(imaplib.IMAP4.readonly, imaplib.IMAP4.abort)
assert issubclass(imaplib.IMAP4_SSL, imaplib.IMAP4)

# Int2AP
assert imaplib.Int2AP(0) == b""
assert imaplib.Int2AP(1) == b"B"
assert imaplib.Int2AP(16) == b"BA"
assert imaplib.Int2AP(65535) == b"PPPP"
assert imaplib.Int2AP(-4096) == b"BAAA"

# ParseFlags
assert imaplib.ParseFlags(b'1 (FLAGS (\\Seen \\Answered $Important))') == (
    b"\\Seen",
    b"\\Answered",
    b"$Important",
)
assert imaplib.ParseFlags(b"1 (FLAGS ())") == ()
assert imaplib.ParseFlags(b"no flags here") == ()

# Time2Internaldate & Internaldate2tuple
dt_utc = datetime.datetime(2024, 3, 15, 10, 20, 30, tzinfo=datetime.timezone.utc)
assert imaplib.Time2Internaldate(dt_utc) == '"15-Mar-2024 10:20:30 +0000"'

dt_plus = datetime.datetime(
    2024, 7, 4, 9, 5, 1, tzinfo=datetime.timezone(datetime.timedelta(hours=5, minutes=30))
)
assert imaplib.Time2Internaldate(dt_plus) == '"04-Jul-2024 09:05:01 +0530"'

# Already-quoted string passes through
assert imaplib.Time2Internaldate('"01-Jan-2024 00:00:00 +0000"') == '"01-Jan-2024 00:00:00 +0000"'

# Naive datetime raises ValueError
try:
    imaplib.Time2Internaldate(datetime.datetime(2024, 1, 1, 0, 0, 0))
    assert False
except ValueError:
    pass

# Unknown type raises ValueError
try:
    imaplib.Time2Internaldate("unquoted")
    assert False
except ValueError:
    pass

# Internaldate2tuple roundtrip against UTC epoch
parsed_tm = imaplib.Internaldate2tuple(b'1 (INTERNALDATE "15-Mar-2024 10:20:30 +0000")')
assert parsed_tm is not None
assert isinstance(parsed_tm, time.struct_time)
assert int(time.mktime(parsed_tm)) == int(dt_utc.timestamp())
assert imaplib.Internaldate2tuple(b"invalid response") is None

# Numeric epoch roundtrip through Time2Internaldate -> Internaldate2tuple
ep_str = imaplib.Time2Internaldate(1710498030)
ep_tm = imaplib.Internaldate2tuple(b"1 (INTERNALDATE " + ep_str.encode("ascii") + b")")
assert ep_tm is not None and int(time.mktime(ep_tm)) == 1710498030

print("poplib_imaplib_unit_ok")

# ===========================================================================
# 2. Offline loopback POP3 + STLS + POP3_SSL mock server parity tests
# ===========================================================================
_POP3_SERVER_CODE = r'''
import select
import socket
import ssl
import sys

cert_file = sys.argv[1]
key_file = sys.argv[2]

srv_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
srv_ctx.load_cert_chain(cert_file, key_file)

plain_srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
plain_srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
plain_srv.bind(("127.0.0.1", 0))
plain_srv.listen(5)
plain_port = plain_srv.getsockname()[1]

tls_srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
tls_srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
tls_srv.bind(("127.0.0.1", 0))
tls_srv.listen(5)
tls_port = tls_srv.getsockname()[1]

sys.stdout.write(f"READY {plain_port} {tls_port}\n")
sys.stdout.flush()

MSGS = {
    1: [b"From: a@example.com", b"Subject: Msg 1", b"", b"Body line 1", b".dotted line"],
    2: [b"From: b@example.com", b"Subject: Msg 2", b"", b"Body two"],
}
UIDS = {1: b"uid-001", 2: b"uid-002"}

def send_line(sock, line):
    sock.sendall(line + b"\r\n")

def send_multiline(sock, header, lines):
    send_line(sock, header)
    for ln in lines:
        if ln.startswith(b"."):
            send_line(sock, b"." + ln)
        else:
            send_line(sock, ln)
    send_line(sock, b".")

def handle_pop3(conn, is_tls):
    if is_tls:
        conn = srv_ctx.wrap_socket(conn, server_side=True)
    rfile = conn.makefile("rb")
    send_line(conn, b"+OK POP3 server ready <1896.697170952@dbc.mtview.ca.us>")
    deleted = set()
    done_all = False

    while True:
        raw = rfile.readline()
        if not raw:
            break
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        parts = line.split()
        if not parts:
            continue
        cmd = parts[0].upper()

        if cmd == "CAPA":
            caps = [b"TOP", b"USER", b"UIDL", b"UTF8", b"IMPLEMENTATION PyRustMock"]
            if not is_tls:
                caps.append(b"STLS")
            send_multiline(conn, b"+OK Capability list follows", caps)
        elif cmd == "STLS":
            send_line(conn, b"+OK Begin TLS negotiation")
            rfile.close()
            conn = srv_ctx.wrap_socket(conn, server_side=True)
            rfile = conn.makefile("rb")
            is_tls = True
        elif cmd == "USER":
            send_line(conn, b"+OK User accepted")
        elif cmd == "PASS":
            if len(parts) > 1 and parts[1] == "badpass":
                send_line(conn, b"-ERR Authentication failed")
            else:
                send_line(conn, b"+OK Mailbox locked and ready")
        elif cmd == "APOP":
            # RFC 1939 test vector: <1896.697170952@dbc.mtview.ca.us> + tanstaaf
            if len(parts) == 3 and parts[1] == "mrose" and parts[2] == "c4c9334bac560ecc979e58001b3e22fb":
                send_line(conn, b"+OK maildrop locked and ready")
            else:
                send_line(conn, b"-ERR permission denied")
        elif cmd == "RPOP":
            send_line(conn, b"+OK RPOP accepted")
        elif cmd == "UTF8":
            send_line(conn, b"+OK UTF8 enabled")
        elif cmd == "STAT":
            active = [k for k in MSGS if k not in deleted]
            total = sum(sum(len(l) + 2 for l in MSGS[k]) for k in active)
            send_line(conn, f"+OK {len(active)} {total}".encode("ascii"))
        elif cmd == "LIST":
            if len(parts) > 1:
                n = int(parts[1])
                sz = sum(len(l) + 2 for l in MSGS[n])
                send_line(conn, f"+OK {n} {sz}".encode("ascii"))
            else:
                entries = [
                    f"{k} {sum(len(l) + 2 for l in MSGS[k])}".encode("ascii")
                    for k in sorted(MSGS)
                    if k not in deleted
                ]
                send_multiline(conn, b"+OK scan listing follows", entries)
        elif cmd == "RETR":
            n = int(parts[1])
            send_multiline(conn, b"+OK message follows", MSGS[n])
        elif cmd == "TOP":
            n = int(parts[1])
            count = int(parts[2])
            msg = MSGS[n]
            sep = msg.index(b"")
            out = msg[: sep + 1 + count]
            send_multiline(conn, b"+OK top follows", out)
        elif cmd == "UIDL":
            if len(parts) > 1:
                n = int(parts[1])
                send_line(conn, f"+OK {n} ".encode("ascii") + UIDS[n])
            else:
                entries = [f"{k} ".encode("ascii") + UIDS[k] for k in sorted(MSGS) if k not in deleted]
                send_multiline(conn, b"+OK uidl follows", entries)
        elif cmd == "DELE":
            deleted.add(int(parts[1]))
            send_line(conn, b"+OK deleted")
        elif cmd == "RSET":
            deleted.clear()
            send_line(conn, b"+OK reset")
        elif cmd == "NOOP":
            send_line(conn, b"+OK")
        elif cmd == "QUIT":
            if len(parts) > 1 and parts[1] == "SHUTDOWN":
                done_all = True
            send_line(conn, b"+OK Bye")
            break
        elif cmd == "XSHUTDOWN":
            send_line(conn, b"+OK Shutting down")
            done_all = True
            break
        else:
            send_line(conn, b"-ERR Unknown command")

    rfile.close()
    conn.close()
    return done_all

while True:
    readable, _, _ = select.select([plain_srv, tls_srv], [], [], 10.0)
    if not readable:
        break
    done = False
    for s in readable:
        c, _ = s.accept()
        if handle_pop3(c, s is tls_srv):
            done = True
            break
    if done:
        break

plain_srv.close()
tls_srv.close()
'''

# ===========================================================================
# 3. Offline loopback IMAP4 + STARTTLS + IMAP4_SSL mock server parity tests
# ===========================================================================
_IMAP_SERVER_CODE = r'''
import base64
import hmac
import select
import socket
import ssl
import sys

cert_file = sys.argv[1]
key_file = sys.argv[2]

srv_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
srv_ctx.load_cert_chain(cert_file, key_file)

plain_srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
plain_srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
plain_srv.bind(("127.0.0.1", 0))
plain_srv.listen(5)
plain_port = plain_srv.getsockname()[1]

tls_srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
tls_srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
tls_srv.bind(("127.0.0.1", 0))
tls_srv.listen(5)
tls_port = tls_srv.getsockname()[1]

sys.stdout.write(f"READY {plain_port} {tls_port}\n")
sys.stdout.flush()

def send_line(sock, data):
    if isinstance(data, str):
        data = data.encode("utf-8")
    sock.sendall(data + b"\r\n")

def handle_imap(conn, is_tls):
    if is_tls:
        conn = srv_ctx.wrap_socket(conn, server_side=True)
    rfile = conn.makefile("rb")
    send_line(conn, b"* OK [CAPABILITY IMAP4rev1 STARTTLS AUTH=CRAM-MD5 ENABLE UTF8=ACCEPT] Mock IMAP4 ready")
    done_all = False
    appended = []

    while True:
        raw = rfile.readline()
        if not raw:
            break
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        parts = line.split(" ")
        if len(parts) < 2:
            continue
        tag = parts[0]
        cmd = parts[1].upper()
        rest = " ".join(parts[2:])

        if cmd == "CAPABILITY":
            if is_tls:
                send_line(conn, "* CAPABILITY IMAP4rev1 AUTH=CRAM-MD5 ENABLE UTF8=ACCEPT SORT THREAD=ORDEREDSUBJECT NAMESPACE ACL QUOTA")
            else:
                send_line(conn, "* CAPABILITY IMAP4rev1 STARTTLS AUTH=CRAM-MD5 ENABLE UTF8=ACCEPT SORT THREAD=ORDEREDSUBJECT NAMESPACE ACL QUOTA")
            send_line(conn, f"{tag} OK CAPABILITY completed")
        elif cmd == "STARTTLS":
            send_line(conn, f"{tag} OK Begin TLS negotiation now")
            rfile.close()
            conn = srv_ctx.wrap_socket(conn, server_side=True)
            rfile = conn.makefile("rb")
            is_tls = True
        elif cmd == "LOGIN":
            if '"secret"' in rest:
                send_line(conn, f"{tag} OK LOGIN completed")
            else:
                send_line(conn, f"{tag} NO Authentication failed")
        elif cmd == "AUTHENTICATE":
            mech = rest.strip().upper()
            if mech == "CRAM-MD5":
                challenge = b"<imap.123@mock.local>"
                send_line(conn, "+ " + base64.b64encode(challenge).decode("ascii"))
                ans_b64 = rfile.readline().strip()
                dec = base64.b64decode(ans_b64).decode("utf-8")
                expected = "alice " + hmac.HMAC(b"secret", challenge, "md5").hexdigest()
                if dec == expected:
                    send_line(conn, f"{tag} OK AUTHENTICATE completed")
                else:
                    send_line(conn, f"{tag} NO AUTHENTICATE failed")
            else:
                send_line(conn, f"{tag} NO Unsupported mechanism")
        elif cmd == "ENABLE":
            send_line(conn, f"* ENABLED {rest}")
            send_line(conn, f"{tag} OK ENABLE completed")
        elif cmd == "LIST":
            send_line(conn, '* LIST (\\HasNoChildren) "/" "INBOX"')
            send_line(conn, '* LIST (\\HasNoChildren) "/" "Sent"')
            send_line(conn, f"{tag} OK LIST completed")
        elif cmd == "LSUB":
            send_line(conn, '* LSUB () "/" "INBOX"')
            send_line(conn, f"{tag} OK LSUB completed")
        elif cmd in ("CREATE", "DELETE", "RENAME", "SUBSCRIBE", "UNSUBSCRIBE", "CHECK", "COPY", "PROXYAUTH", "SETACL", "DELETEACL", "UNSELECT"):
            send_line(conn, f"{tag} OK {cmd} completed")
        elif cmd == "STATUS":
            send_line(conn, "* STATUS INBOX (MESSAGES 2 RECENT 1)")
            send_line(conn, f"{tag} OK STATUS completed")
        elif cmd in ("SELECT", "EXAMINE"):
            send_line(conn, "* 2 EXISTS")
            send_line(conn, "* 1 RECENT")
            send_line(conn, "* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)")
            send_line(conn, "* OK [UIDVALIDITY 3857529045] UIDs valid")
            if cmd == "EXAMINE" or "READONLY_MBOX" in rest:
                send_line(conn, f"{tag} OK [READ-ONLY] {cmd} completed")
            else:
                send_line(conn, f"{tag} OK [READ-WRITE] {cmd} completed")
        elif cmd == "NOOP":
            send_line(conn, f"{tag} OK NOOP completed")
        elif cmd == "SEARCH":
            send_line(conn, "* SEARCH 1 2")
            send_line(conn, f"{tag} OK SEARCH completed")
        elif cmd == "FETCH":
            body = b"From: alice@example.com\r\nSubject: Hi\r\n\r\nHello IMAP!"
            send_line(conn, f"* 1 FETCH (RFC822 {{{len(body)}}}")
            conn.sendall(body)
            send_line(conn, ")")
            send_line(conn, f"{tag} OK FETCH completed")
        elif cmd == "STORE":
            send_line(conn, "* 1 FETCH (FLAGS (\\Seen))")
            send_line(conn, f"{tag} OK STORE completed")
        elif cmd == "EXPUNGE":
            send_line(conn, "* 2 EXPUNGE")
            send_line(conn, f"{tag} OK EXPUNGE completed")
        elif cmd == "APPEND":
            # Parse trailing {N} literal size
            lbrace = line.rfind("{")
            rbrace = line.rfind("}")
            size = int(line[lbrace + 1 : rbrace])
            send_line(conn, "+ Ready for literal data")
            lit = rfile.read(size)
            # Consume trailing CRLF
            rfile.readline()
            appended.append(lit)
            send_line(conn, f"{tag} OK APPEND completed ({len(lit)} bytes)")
        elif cmd == "UID":
            sub = parts[2].upper()
            if sub == "SEARCH":
                send_line(conn, "* SEARCH 101 102")
            else:
                send_line(conn, "* 1 FETCH (UID 101 FLAGS (\\Seen))")
            send_line(conn, f"{tag} OK UID completed")
        elif cmd == "SORT":
            send_line(conn, "* SORT 2 1")
            send_line(conn, f"{tag} OK SORT completed")
        elif cmd == "THREAD":
            send_line(conn, "* THREAD (1)(2)")
            send_line(conn, f"{tag} OK THREAD completed")
        elif cmd == "GETACL":
            send_line(conn, "* ACL INBOX alice lrswipkxtecda")
            send_line(conn, f"{tag} OK GETACL completed")
        elif cmd == "MYRIGHTS":
            send_line(conn, "* MYRIGHTS INBOX lrswipkxtecda")
            send_line(conn, f"{tag} OK MYRIGHTS completed")
        elif cmd == "GETQUOTA":
            send_line(conn, '* QUOTA "" (STORAGE 10 512)')
            send_line(conn, f"{tag} OK GETQUOTA completed")
        elif cmd == "GETQUOTAROOT":
            send_line(conn, '* QUOTAROOT INBOX ""')
            send_line(conn, '* QUOTA "" (STORAGE 10 512)')
            send_line(conn, f"{tag} OK GETQUOTAROOT completed")
        elif cmd == "SETQUOTA":
            send_line(conn, '* QUOTA "" (STORAGE 10 1024)')
            send_line(conn, f"{tag} OK SETQUOTA completed")
        elif cmd in ("GETANNOTATION", "SETANNOTATION"):
            send_line(conn, '* ANNOTATION INBOX "/comment" ("value.priv" "note")')
            send_line(conn, f"{tag} OK {cmd} completed")
        elif cmd == "NAMESPACE":
            send_line(conn, '* NAMESPACE (("" "/")) NIL NIL')
            send_line(conn, f"{tag} OK NAMESPACE completed")
        elif cmd == "PARTIAL":
            send_line(conn, "* 1 FETCH (BODY[TEXT] \"Hel\")")
            send_line(conn, f"{tag} OK PARTIAL completed")
        elif cmd == "XCUSTOM":
            send_line(conn, f"{tag} OK XCUSTOM {rest}")
        elif cmd == "XSHUTDOWN":
            send_line(conn, "* BYE Server shutting down")
            send_line(conn, f"{tag} OK XSHUTDOWN completed")
            done_all = True
            break
        elif cmd == "CLOSE":
            send_line(conn, f"{tag} OK CLOSE completed")
        elif cmd == "LOGOUT":
            send_line(conn, "* BYE IMAP4rev1 Server logging out")
            send_line(conn, f"{tag} OK LOGOUT completed")
            break
        else:
            send_line(conn, f"{tag} BAD Unknown command {cmd}")

    rfile.close()
    conn.close()
    return done_all

while True:
    readable, _, _ = select.select([plain_srv, tls_srv], [], [], 10.0)
    if not readable:
        break
    done = False
    for s in readable:
        c, _ = s.accept()
        if handle_imap(c, s is tls_srv):
            done = True
            break
    if done:
        break

plain_srv.close()
tls_srv.close()
'''

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

    cli_ctx = ssl.create_default_context(cafile=ca_file)

    # --- Run POP3 / STLS / POP3_SSL parity tests ---
    pop_proc = subprocess.Popen(
        [sys.executable, "-c", _POP3_SERVER_CODE, cert_file, key_file],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        ready = pop_proc.stdout.readline().decode("ascii").strip()
        assert ready.startswith("READY "), f"unexpected POP3 server line: {ready!r}"
        _, p_plain, p_tls = ready.split()
        pop_port = int(p_plain)
        pop_ssl_port = int(p_tls)

        # 2a. POP3 plain + STLS upgrade + full command set
        p = poplib.POP3("127.0.0.1", pop_port, timeout=5)
        try:
            assert p.getwelcome().startswith(b"+OK POP3 server ready")
            p.set_debuglevel(0)
            caps = p.capa()
            assert "STLS" in caps and "UIDL" in caps and caps["IMPLEMENTATION"] == ["PyRustMock"]

            # Upgrade via STLS
            assert p.stls(context=cli_ctx).startswith(b"+OK")
            assert isinstance(p.sock, ssl.SSLSocket)
            # Second stls() must fail
            try:
                p.stls(context=cli_ctx)
                assert False
            except poplib.error_proto:
                pass

            assert p.user("alice").startswith(b"+OK")
            try:
                p.pass_("badpass")
                assert False
            except poplib.error_proto as err:
                assert b"-ERR" in err.args[0]
            assert p.pass_("goodpass").startswith(b"+OK")
            assert p.utf8().startswith(b"+OK")
            assert p.rpop("alice").startswith(b"+OK")
            assert p.noop().startswith(b"+OK")

            count, total_size = p.stat()
            assert count == 2 and total_size > 0

            resp, listing, _ = p.list()
            assert resp.startswith(b"+OK") and len(listing) == 2
            assert p.list(1).startswith(b"+OK 1 ")

            resp, lines, octets = p.retr(1)
            assert resp.startswith(b"+OK")
            assert lines == [
                b"From: a@example.com",
                b"Subject: Msg 1",
                b"",
                b"Body line 1",
                b".dotted line",
            ]
            assert octets == sum(len(x) + 2 for x in lines)

            resp, top_lines, _ = p.top(1, 1)
            assert resp.startswith(b"+OK")
            assert top_lines == [b"From: a@example.com", b"Subject: Msg 1", b"", b"Body line 1"]

            resp, uids, _ = p.uidl()
            assert uids == [b"1 uid-001", b"2 uid-002"]
            assert p.uidl(1) == b"+OK 1 uid-001"

            assert p.dele(2).startswith(b"+OK")
            assert p.stat()[0] == 1
            assert p.rset().startswith(b"+OK")
            assert p.stat()[0] == 2
            assert p.quit().startswith(b"+OK")
        finally:
            p.close()

        # 2b. POP3 APOP RFC 1939 MD5 digest test
        p_apop = poplib.POP3("127.0.0.1", pop_port, timeout=5)
        try:
            assert p_apop.apop("mrose", "tanstaaf").startswith(b"+OK")
            assert p_apop.quit().startswith(b"+OK")
        finally:
            p_apop.close()

        # 2c. POP3_SSL implicit TLS + clean server shutdown
        ps = poplib.POP3_SSL("127.0.0.1", pop_ssl_port, timeout=5, context=cli_ctx)
        try:
            assert isinstance(ps.sock, ssl.SSLSocket)
            assert ps.getwelcome().startswith(b"+OK")
            assert ps.stat()[0] == 2
            try:
                ps.stls()
                assert False
            except poplib.error_proto:
                pass
            assert ps._shortcmd("XSHUTDOWN").startswith(b"+OK")
        finally:
            ps.close()

        print("poplib_loopback_ok")
    finally:
        try:
            pop_proc.wait(timeout=10)
        except Exception:
            pop_proc.kill()
            pop_proc.wait(timeout=5)
        if pop_proc.stdout:
            pop_proc.stdout.close()
        if pop_proc.stderr:
            pop_proc.stderr.close()

    assert pop_proc.returncode == 0

    # --- Run IMAP4 / STARTTLS / IMAP4_SSL parity tests ---
    imap_proc = subprocess.Popen(
        [sys.executable, "-c", _IMAP_SERVER_CODE, cert_file, key_file],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        ready = imap_proc.stdout.readline().decode("ascii").strip()
        assert ready.startswith("READY "), f"unexpected IMAP server line: {ready!r}"
        _, i_plain, i_tls = ready.split()
        imap_port = int(i_plain)
        imap_ssl_port = int(i_tls)

        # 3a. IMAP4 + STARTTLS + LOGIN + full command suite
        with imaplib.IMAP4("127.0.0.1", imap_port, timeout=5) as m:
            assert m.PROTOCOL_VERSION == "IMAP4REV1"
            assert "STARTTLS" in m.capabilities
            assert m.state == "NONAUTH"
            assert m.socket() is m.sock

            # STARTTLS upgrade refreshes capabilities
            typ, _ = m.starttls(ssl_context=cli_ctx)
            assert typ == "OK"
            assert isinstance(m.sock, ssl.SSLSocket)
            assert "STARTTLS" not in m.capabilities

            # Login
            typ, _ = m.login("alice", "secret")
            assert typ == "OK" and m.state == "AUTH"

            # Enable UTF8=ACCEPT
            typ, _ = m.enable("UTF8=ACCEPT")
            assert typ == "OK" and m.utf8_enabled is True

            # Mailbox listing & management
            typ, mboxes = m.list()
            assert typ == "OK" and len(mboxes) == 2
            typ, subs = m.lsub()
            assert typ == "OK" and len(subs) == 1
            assert m.create("Archive")[0] == "OK"
            assert m.rename("Archive", "Archive2")[0] == "OK"
            assert m.subscribe("Archive2")[0] == "OK"
            assert m.unsubscribe("Archive2")[0] == "OK"
            assert m.delete("Archive2")[0] == "OK"
            typ, st_data = m.status("INBOX", "(MESSAGES RECENT)")
            assert typ == "OK" and b"MESSAGES 2" in st_data[0]
            assert m.proxyauth("bob")[0] == "OK"

            # ACL / Quota / Annotation / Namespace
            assert m.getacl("INBOX")[0] == "OK"
            assert m.setacl("INBOX", "bob", "lr")[0] == "OK"
            assert m.deleteacl("INBOX", "bob")[0] == "OK"
            assert m.myrights("INBOX")[0] == "OK"
            assert m.getquota('""')[0] == "OK"
            typ, qr = m.getquotaroot("INBOX")
            assert typ == "OK" and len(qr) == 2
            assert m.setquota('""', "(STORAGE 1024)")[0] == "OK"
            assert m.getannotation("INBOX", '"/comment"', '"value.priv"')[0] == "OK"
            assert m.setannotation("INBOX", '"/comment"', '("value.priv" "v")')[0] == "OK"
            assert m.namespace()[0] == "OK"

            # Select readonly mailbox without readonly=True raises IMAP4.readonly
            try:
                m.select("READONLY_MBOX")
                assert False
            except imaplib.IMAP4.readonly:
                pass

            # Select INBOX
            typ, data = m.select("INBOX")
            assert typ == "OK" and data == [b"2"] and m.state == "SELECTED"
            assert m.response("UIDVALIDITY") == ("UIDVALIDITY", [b"3857529045"])
            assert m.recent() == ("OK", [b"1"])

            # Uppercase command dispatch via __getattr__
            assert m.NOOP()[0] == "OK"

            # Search, Fetch with literal, Store, Copy, Append, UID, Sort, Thread, Partial, Expunge, Check
            assert m.search(None, "ALL") == ("OK", [b"1 2"])
            typ, fetched = m.fetch("1", "(RFC822)")
            assert typ == "OK"
            assert isinstance(fetched[0], tuple)
            assert fetched[0][1] == b"From: alice@example.com\r\nSubject: Hi\r\n\r\nHello IMAP!"

            assert m.store("1", "+FLAGS", "\\Seen")[0] == "OK"
            assert m.copy("1", "Sent")[0] == "OK"
            assert m.append("INBOX", "\\Seen", dt_utc, b"Subject: Appended\n\nHi")[0] == "OK"
            assert m.uid("SEARCH", "ALL") == ("OK", [b"101 102"])
            assert m.uid("FETCH", "101", "(FLAGS)")[0] == "OK"
            assert m.sort("DATE", "UTF-8", "ALL") == ("OK", [b"2 1"])
            assert m.thread("ORDEREDSUBJECT", "UTF-8", "ALL") == ("OK", [b"(1)(2)"])
            assert m.partial("1", "BODY[TEXT]", "1", "3")[0] == "OK"
            assert m.check()[0] == "OK"
            assert m.expunge() == ("OK", [b"2"])
            assert m.xatom("XCUSTOM", "arg1")[0] == "OK"

            # Unselect & re-select & close
            assert m.unselect()[0] == "OK" and m.state == "AUTH"
            assert m.select("INBOX", readonly=True)[0] == "OK"
            assert m.close()[0] == "OK" and m.state == "AUTH"

        print("imaplib_starttls_ok")

        # 3b. IMAP4 CRAM-MD5 authentication
        with imaplib.IMAP4("127.0.0.1", imap_port, timeout=5) as m2:
            typ, _ = m2.login_cram_md5("alice", "secret")
            assert typ == "OK" and m2.state == "AUTH"

        # 3c. IMAP4_SSL implicit TLS + clean server shutdown
        m_ssl = imaplib.IMAP4_SSL("127.0.0.1", imap_ssl_port, ssl_context=cli_ctx, timeout=5)
        try:
            assert isinstance(m_ssl.sock, ssl.SSLSocket)
            assert m_ssl.login("alice", "secret")[0] == "OK"
            assert m_ssl.select("INBOX") == ("OK", [b"2"])
            typ, fetched = m_ssl.fetch("1", "(RFC822)")
            assert typ == "OK" and fetched[0][1].endswith(b"Hello IMAP!")
            try:
                m_ssl.xatom("XSHUTDOWN")
            except imaplib.IMAP4.abort:
                pass
        finally:
            m_ssl.shutdown()

        print("imaplib_ssl_ok")
    finally:
        try:
            imap_proc.wait(timeout=10)
        except Exception:
            imap_proc.kill()
            imap_proc.wait(timeout=5)
        if imap_proc.stdout:
            imap_proc.stdout.close()
        if imap_proc.stderr:
            imap_proc.stderr.close()

    assert imap_proc.returncode == 0

print("poplib_imaplib_all_ok")
