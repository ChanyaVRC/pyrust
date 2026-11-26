"""Comprehensive 100% offline deterministic CPython 3.12 parity tests for
ftplib (FTP, FTP_TLS) and smtplib (SMTP, SMTP_SSL, LMTP) (issue #3258).
"""

import email
import email.message
import ftplib
import io
import os
import smtplib
import socket
import ssl
import subprocess
import sys
import tempfile

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
# 1. ftplib constants, exception hierarchy, and parsers
# ===========================================================================
assert ftplib.FTP_PORT == 21
assert ftplib.MAXLINE == 8192
assert ftplib.CRLF == "\r\n"
assert ftplib.B_CRLF == b"\r\n"
assert ftplib.MSG_OOB == 1

assert issubclass(ftplib.Error, Exception)
assert issubclass(ftplib.error_reply, ftplib.Error)
assert issubclass(ftplib.error_temp, ftplib.Error)
assert issubclass(ftplib.error_perm, ftplib.Error)
assert issubclass(ftplib.error_proto, ftplib.Error)
assert ftplib.Error in ftplib.all_errors
assert OSError in ftplib.all_errors
assert EOFError in ftplib.all_errors
assert ssl.SSLError in ftplib.all_errors
assert issubclass(ftplib.FTP_TLS, ftplib.FTP)

# parse150
assert ftplib.parse150("150 Opening BINARY mode data connection for foo (1234 bytes).") == 1234
assert ftplib.parse150("150 Opening ASCII mode data connection.") is None
try:
    ftplib.parse150("200 OK")
    assert False
except ftplib.error_reply:
    pass

# parse227
assert ftplib.parse227("227 Entering Passive Mode (127,0,0,1,4,1).") == ("127.0.0.1", 1025)
try:
    ftplib.parse227("200 OK")
    assert False
except ftplib.error_reply:
    pass
try:
    ftplib.parse227("227 no numbers here")
    assert False
except ftplib.error_proto:
    pass

# parse229
assert ftplib.parse229("229 Entering Extended Passive Mode (|||6446|)", ("127.0.0.1", 21)) == (
    "127.0.0.1",
    6446,
)
try:
    ftplib.parse229("229 bad", ("127.0.0.1", 21))
    assert False
except ftplib.error_proto:
    pass

# parse257
assert ftplib.parse257('257 "/pub/dir" is current directory.') == "/pub/dir"
assert ftplib.parse257('257 "/pub/""quoted""/dir" created.') == '/pub/"quoted"/dir'
assert ftplib.parse257("257 /unquoted") == ""
try:
    ftplib.parse257("250 OK")
    assert False
except ftplib.error_reply:
    pass

# sanitize & newline guard
_tmp_ftp = ftplib.FTP()
assert _tmp_ftp.sanitize("PASS secret\r\n") == "'PASS ******\\r\\n'"
assert _tmp_ftp.sanitize("USER alice\r\n") == "'USER alice\\r\\n'"
try:
    _tmp_ftp.putline("NOOP\r\nQUIT")
    assert False
except ValueError:
    pass

print("ftplib_unit_ok")

# ===========================================================================
# 2. smtplib constants, exception hierarchy, and helpers
# ===========================================================================
assert smtplib.SMTP_PORT == 25
assert smtplib.SMTP_SSL_PORT == 465
assert smtplib.LMTP_PORT == 2003
assert smtplib.CRLF == "\r\n"
assert smtplib.bCRLF == b"\r\n"

assert issubclass(smtplib.SMTPException, OSError)
assert issubclass(smtplib.SMTPNotSupportedError, smtplib.SMTPException)
assert issubclass(smtplib.SMTPServerDisconnected, smtplib.SMTPException)
assert issubclass(smtplib.SMTPResponseException, smtplib.SMTPException)
assert issubclass(smtplib.SMTPSenderRefused, smtplib.SMTPResponseException)
assert issubclass(smtplib.SMTPRecipientsRefused, smtplib.SMTPException)
assert issubclass(smtplib.SMTPDataError, smtplib.SMTPResponseException)
assert issubclass(smtplib.SMTPConnectError, smtplib.SMTPResponseException)
assert issubclass(smtplib.SMTPHeloError, smtplib.SMTPResponseException)
assert issubclass(smtplib.SMTPAuthenticationError, smtplib.SMTPResponseException)
assert issubclass(smtplib.SMTP_SSL, smtplib.SMTP)
assert issubclass(smtplib.LMTP, smtplib.SMTP)

rexc = smtplib.SMTPResponseException(550, b"No such user")
assert rexc.smtp_code == 550 and rexc.smtp_error == b"No such user"
assert rexc.args == (550, b"No such user")

sexc = smtplib.SMTPSenderRefused(550, b"Denied", "bad@example.com")
assert sexc.smtp_code == 550 and sexc.smtp_error == b"Denied" and sexc.sender == "bad@example.com"

rrexc = smtplib.SMTPRecipientsRefused({"a@b.com": (550, b"Unknown")})
assert rrexc.recipients == {"a@b.com": (550, b"Unknown")}

assert smtplib.quoteaddr("Alice <alice@example.com>") == "<alice@example.com>"
assert smtplib.quoteaddr("bob@example.com") == "<bob@example.com>"
assert smtplib.quoteaddr("") == "<>"
assert smtplib.quotedata("line1\n.line2\r.line3\r\nend") == "line1\r\n..line2\r\n..line3\r\nend"

print("smtplib_unit_ok")

# ===========================================================================
# 3. Offline loopback FTP + FTP_TLS mock server parity tests
# ===========================================================================
_FTP_SERVER_CODE = r'''
import socket
import ssl
import sys

cert_file = sys.argv[1]
key_file = sys.argv[2]

srv_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
srv_ctx.load_cert_chain(cert_file, key_file)

files = {
    "hello.txt": b"Hello FTP world!\r\nSecond line\r\n",
    "bin.dat": b"\x00\x01\x02\x03BINARY_PAYLOAD_XYZ",
}
dirs = {"/"}
cwd = "/"

ctrl_srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
ctrl_srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
ctrl_srv.bind(("127.0.0.1", 0))
ctrl_srv.listen(5)
port = ctrl_srv.getsockname()[1]

sys.stdout.write(f"READY {port}\n")
sys.stdout.flush()

def send_line(sock, line):
    sock.sendall((line + "\r\n").encode("utf-8"))

def handle_client(conn):
    global cwd
    rfile = conn.makefile("r", encoding="utf-8")
    send_line(conn, "220-PyRust Mock FTP")
    send_line(conn, "220 Ready")
    pasv_srv = None
    active_addr = None
    prot_p = False
    rename_from = None
    done_all = False

    def open_data():
        nonlocal pasv_srv, active_addr
        if pasv_srv is not None:
            dconn, _ = pasv_srv.accept()
            pasv_srv.close()
            pasv_srv = None
        else:
            dconn = socket.create_connection(active_addr, timeout=5)
            active_addr = None
        if prot_p:
            dconn = srv_ctx.wrap_socket(dconn, server_side=True)
        return dconn

    def close_data(dconn):
        if isinstance(dconn, ssl.SSLSocket):
            try:
                raw = dconn.unwrap()
                raw.close()
            except Exception:
                dconn.close()
        else:
            dconn.close()

    while True:
        line = rfile.readline()
        if not line:
            break
        line = line.rstrip("\r\n")
        parts = line.split(" ", 1)
        cmd = parts[0].upper()
        arg = parts[1] if len(parts) > 1 else ""

        if cmd == "USER":
            if arg == "acctuser":
                send_line(conn, "331 Password required")
            else:
                send_line(conn, "331 User OK")
        elif cmd == "PASS":
            if arg == "needacct":
                send_line(conn, "332 Need account")
            else:
                send_line(conn, "230 Login successful")
        elif cmd == "ACCT":
            send_line(conn, "230 Account accepted")
        elif cmd == "AUTH":
            send_line(conn, "234 AUTH TLS OK")
            rfile.close()
            conn = srv_ctx.wrap_socket(conn, server_side=True)
            rfile = conn.makefile("r", encoding="utf-8")
        elif cmd == "CCC":
            send_line(conn, "200 Clearing control channel")
            rfile.close()
            conn = conn.unwrap()
            rfile = conn.makefile("r", encoding="utf-8")
        elif cmd == "PBSZ":
            send_line(conn, "200 PBSZ=0")
        elif cmd == "PROT":
            prot_p = (arg.upper() == "P")
            send_line(conn, f"200 PROT {arg.upper()} OK")
        elif cmd == "TYPE":
            send_line(conn, f"200 Type set to {arg}")
        elif cmd == "NOOP":
            send_line(conn, "200 NOOP ok")
        elif cmd == "PWD":
            send_line(conn, f'257 "{cwd}" is current directory.')
        elif cmd == "CWD":
            cwd = arg if arg.startswith("/") else ("/" + arg)
            send_line(conn, "250 Directory changed")
        elif cmd == "CDUP":
            cwd = "/"
            send_line(conn, "250 CDUP ok")
        elif cmd == "MKD":
            dirs.add(arg)
            send_line(conn, f'257 "{arg}" created')
        elif cmd == "RMD":
            dirs.discard(arg)
            send_line(conn, "250 Directory removed")
        elif cmd == "SIZE":
            if arg in files:
                send_line(conn, f"213 {len(files[arg])}")
            else:
                send_line(conn, "550 File not found")
        elif cmd == "RNFR":
            rename_from = arg
            send_line(conn, "350 Ready for RNTO")
        elif cmd == "RNTO":
            if rename_from in files:
                files[arg] = files.pop(rename_from)
            rename_from = None
            send_line(conn, "250 Rename ok")
        elif cmd == "DELE":
            if arg in files:
                del files[arg]
                send_line(conn, "250 Deleted")
            else:
                send_line(conn, "550 Not found")
        elif cmd == "TEMPERR":
            send_line(conn, "421 Temporary failure")
        elif cmd == "BADPROTO":
            send_line(conn, "999 Weird reply")
        elif cmd == "PASV":
            if pasv_srv is not None:
                pasv_srv.close()
            pasv_srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            pasv_srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            pasv_srv.bind(("127.0.0.1", 0))
            pasv_srv.listen(1)
            dp = pasv_srv.getsockname()[1]
            send_line(conn, f"227 Entering Passive Mode (127,0,0,1,{dp // 256},{dp % 256}).")
        elif cmd == "PORT":
            nums = [int(x) for x in arg.split(",")]
            active_addr = (".".join(str(x) for x in nums[:4]), nums[4] * 256 + nums[5])
            send_line(conn, "200 PORT ok")
        elif cmd == "REST":
            conn_rest = int(arg)
            send_line(conn, f"350 Restarting at {conn_rest}")
        elif cmd == "OPTS":
            send_line(conn, "200 OPTS ok")
        elif cmd == "RETR":
            data = files.get(arg, b"")
            send_line(conn, f"150 Opening data connection ({len(data)} bytes).")
            dconn = open_data()
            dconn.sendall(data)
            close_data(dconn)
            send_line(conn, "226 Transfer complete.")
        elif cmd == "STOR":
            send_line(conn, "150 Ok to send data.")
            dconn = open_data()
            chunks = []
            while True:
                chunk = dconn.recv(4096)
                if not chunk:
                    break
                chunks.append(chunk)
            files[arg] = b"".join(chunks)
            close_data(dconn)
            send_line(conn, "226 Transfer complete.")
        elif cmd == "NLST":
            send_line(conn, "150 Here comes the list.")
            dconn = open_data()
            body = "".join(k + "\r\n" for k in sorted(files)).encode("utf-8")
            dconn.sendall(body)
            close_data(dconn)
            send_line(conn, "226 Directory send OK.")
        elif cmd == "LIST":
            send_line(conn, "150 Listing.")
            dconn = open_data()
            body = "".join(f"-rw-r--r-- 1 user group {len(files[k])} {k}\r\n" for k in sorted(files)).encode("utf-8")
            dconn.sendall(body)
            close_data(dconn)
            send_line(conn, "226 Directory send OK.")
        elif cmd == "MLSD":
            send_line(conn, "150 MLSD listing.")
            dconn = open_data()
            body = "".join(f"type=file;size={len(files[k])}; {k}\r\n" for k in sorted(files)).encode("utf-8")
            dconn.sendall(body)
            close_data(dconn)
            send_line(conn, "226 MLSD complete.")
        elif cmd == "SHUTDOWN_SERVER":
            send_line(conn, "221 Shutting down")
            done_all = True
            break
        elif cmd == "QUIT":
            send_line(conn, "221 Goodbye.")
            break
        else:
            send_line(conn, f"502 Unknown command {cmd}")

    rfile.close()
    conn.close()
    if pasv_srv is not None:
        pasv_srv.close()
    return done_all

while True:
    c, _ = ctrl_srv.accept()
    if handle_client(c):
        break
ctrl_srv.close()
'''

# ===========================================================================
# 4. Offline loopback SMTP + STARTTLS + SMTP_SSL + LMTP mock server
# ===========================================================================
_SMTP_SERVER_CODE = r'''
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

def send_line(sock, line):
    sock.sendall((line + "\r\n").encode("ascii"))

def handle_smtp(conn, is_tls):
    if is_tls:
        conn = srv_ctx.wrap_socket(conn, server_side=True)
    rfile = conn.makefile("rb")
    send_line(conn, "220 mock.smtp.local ESMTP Ready")
    last_data = b""
    done_all = False

    while True:
        raw = rfile.readline()
        if not raw:
            break
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        parts = line.split(" ", 1)
        cmd = parts[0].upper()
        arg = parts[1] if len(parts) > 1 else ""

        if cmd in ("EHLO", "LHLO"):
            send_line(conn, f"250-mock.smtp.local Hello {arg}")
            send_line(conn, "250-SIZE 10485760")
            send_line(conn, "250-AUTH CRAM-MD5 PLAIN LOGIN")
            if not is_tls:
                send_line(conn, "250-STARTTLS")
            send_line(conn, "250 SMTPUTF8")
        elif cmd == "HELO":
            send_line(conn, f"250 mock.smtp.local Hello {arg}")
        elif cmd == "HELP":
            send_line(conn, "214 Mock SMTP help text")
        elif cmd == "NOOP":
            send_line(conn, "250 OK")
        elif cmd == "RSET":
            send_line(conn, "250 Reset OK")
        elif cmd == "VRFY":
            send_line(conn, f"250 <{arg}>")
        elif cmd == "EXPN":
            send_line(conn, f"250 <{arg}>")
        elif cmd == "STARTTLS":
            send_line(conn, "220 Ready to start TLS")
            rfile.close()
            conn = srv_ctx.wrap_socket(conn, server_side=True)
            rfile = conn.makefile("rb")
            is_tls = True
        elif cmd == "AUTH":
            aparts = arg.split(" ", 1)
            mech = aparts[0].upper()
            init = aparts[1] if len(aparts) > 1 else ""
            if mech == "PLAIN":
                if not init:
                    send_line(conn, "334 ")
                    init = rfile.readline().decode("ascii").strip()
                dec = base64.b64decode(init)
                if dec == b"\x00alice\x00secret":
                    send_line(conn, "235 Authentication successful")
                else:
                    send_line(conn, "535 Authentication failed")
            elif mech == "LOGIN":
                if init:
                    u = base64.b64decode(init)
                else:
                    send_line(conn, "334 " + base64.b64encode(b"Username:").decode("ascii"))
                    u = base64.b64decode(rfile.readline().strip())
                send_line(conn, "334 " + base64.b64encode(b"Password:").decode("ascii"))
                p = base64.b64decode(rfile.readline().strip())
                if u == b"alice" and p == b"secret":
                    send_line(conn, "235 Authentication successful")
                else:
                    send_line(conn, "535 Authentication failed")
            elif mech == "CRAM-MD5":
                challenge = b"<1234.5678@mock.smtp.local>"
                send_line(conn, "334 " + base64.b64encode(challenge).decode("ascii"))
                resp_b64 = rfile.readline().strip()
                dec = base64.b64decode(resp_b64).decode("ascii")
                expected = "alice " + hmac.HMAC(b"secret", challenge, "md5").hexdigest()
                if dec == expected:
                    send_line(conn, "235 CRAM-MD5 OK")
                else:
                    send_line(conn, "535 Bad CRAM-MD5")
            else:
                send_line(conn, "504 Unrecognized auth type")
        elif cmd == "MAIL":
            if "refused@" in arg:
                send_line(conn, "550 Sender refused")
            else:
                send_line(conn, "250 Sender OK")
        elif cmd == "RCPT":
            if "bad@" in arg:
                send_line(conn, "550 No such user")
            else:
                send_line(conn, "250 Recipient OK")
        elif cmd == "DATA":
            send_line(conn, "354 End data with <CR><LF>.<CR><LF>")
            lines = []
            while True:
                dline = rfile.readline()
                if not dline or dline == b".\r\n":
                    break
                if dline.startswith(b".."):
                    dline = dline[1:]
                lines.append(dline)
            last_data = b"".join(lines)
            if b"TRIGGER_DATA_ERR" in last_data:
                send_line(conn, "554 Message rejected")
            elif b"Bcc:" in last_data:
                send_line(conn, "554 Bcc header leaked")
            else:
                send_line(conn, f"250 Queued ({len(last_data)} bytes)")
        elif cmd == "XLAST":
            send_line(conn, "250 " + base64.b64encode(last_data).decode("ascii"))
        elif cmd == "XSHUTDOWN":
            send_line(conn, "221 Bye")
            done_all = True
            break
        elif cmd == "QUIT":
            send_line(conn, "221 Bye")
            break
        else:
            send_line(conn, f"500 Unknown {cmd}")

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
        if handle_smtp(c, s is tls_srv):
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

    # --- Run FTP / FTP_TLS parity tests ---
    ftp_proc = subprocess.Popen(
        [sys.executable, "-c", _FTP_SERVER_CODE, cert_file, key_file],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        ready = ftp_proc.stdout.readline().decode("ascii").strip()
        assert ready.startswith("READY "), f"unexpected FTP server line: {ready!r}"
        ftp_port = int(ready.split()[1])

        # 3a. Plain FTP (PASV + active PORT + all commands)
        with ftplib.FTP(timeout=5) as ftp:
            w = ftp.connect("127.0.0.1", ftp_port)
            assert "PyRust Mock FTP" in w and "220 Ready" in w
            assert ftp.getwelcome() == w
            ftp.set_debuglevel(0)
            assert ftp.login("acctuser", "needacct", "myacct").startswith("230")
            assert ftp.pwd() == "/"
            assert ftp.mkd("/sub") == "/sub"
            assert ftp.cwd("/sub").startswith("250")
            assert ftp.pwd() == "/sub"
            assert ftp.cwd("..").startswith("250")
            assert ftp.pwd() == "/"
            assert ftp.rmd("/sub").startswith("250")
            assert ftp.size("bin.dat") == 22

            # retrbinary (PASV)
            got_bin = bytearray()
            assert ftp.retrbinary("RETR bin.dat", got_bin.extend).startswith("226")
            assert bytes(got_bin) == b"\x00\x01\x02\x03BINARY_PAYLOAD_XYZ"

            # retrlines (PASV)
            got_lines = []
            assert ftp.retrlines("RETR hello.txt", got_lines.append).startswith("226")
            assert got_lines == ["Hello FTP world!", "Second line"]

            # storbinary + callback (PASV)
            cb_chunks = []
            payload = io.BytesIO(b"uploaded-binary-12345")
            assert ftp.storbinary("STOR up.bin", payload, blocksize=8, callback=cb_chunks.append).startswith("226")
            assert b"".join(cb_chunks) == b"uploaded-binary-12345"
            assert ftp.size("up.bin") == 21

            # storlines (active PORT mode!)
            ftp.set_pasv(False)
            line_cb = []
            assert ftp.storlines("STOR up.txt", io.BytesIO(b"alpha\nbeta\r\n"), callback=line_cb.append).startswith("226")
            assert line_cb == [b"alpha\r\n", b"beta\r\n"]
            back_lines = []
            ftp.retrlines("RETR up.txt", back_lines.append)
            assert back_lines == ["alpha", "beta"]
            ftp.set_pasv(True)

            # nlst, dir, mlsd
            names = ftp.nlst()
            assert "hello.txt" in names and "up.bin" in names and "up.txt" in names
            dir_lines = []
            ftp.dir(dir_lines.append)
            assert any("hello.txt" in ln for ln in dir_lines)
            mlsd_entries = dict(ftp.mlsd(facts=["type", "size"]))
            assert mlsd_entries["bin.dat"]["type"] == "file"
            assert mlsd_entries["bin.dat"]["size"] == "22"

            # rename & delete
            assert ftp.rename("up.bin", "moved.bin").startswith("250")
            assert ftp.delete("moved.bin").startswith("250")
            assert ftp.acct("extra").startswith("230")

            # error classes
            try:
                ftp.size("missing.dat")
                assert False
            except ftplib.error_perm:
                pass
            try:
                ftp.sendcmd("TEMPERR")
                assert False
            except ftplib.error_temp:
                pass
            try:
                ftp.sendcmd("BADPROTO")
                assert False
            except ftplib.error_proto:
                pass

        print("ftplib_plain_ok")

        # 3b. FTP_TLS (AUTH TLS, PROT P encrypted data, PROT C, CCC cleartext control)
        with ftplib.FTP_TLS(context=cli_ctx, timeout=5) as ftps:
            ftps.connect("127.0.0.1", ftp_port)
            assert ftps.login("alice", "pw").startswith("230")
            assert isinstance(ftps.sock, ssl.SSLSocket)
            assert ftps.prot_p().startswith("200")

            tls_bin = bytearray()
            assert ftps.retrbinary("RETR bin.dat", tls_bin.extend).startswith("226")
            assert bytes(tls_bin) == b"\x00\x01\x02\x03BINARY_PAYLOAD_XYZ"

            assert ftps.storbinary("STOR tls_up.bin", io.BytesIO(b"secret-ftp-tls")).startswith("226")
            check_tls = bytearray()
            ftps.retrbinary("RETR tls_up.bin", check_tls.extend)
            assert bytes(check_tls) == b"secret-ftp-tls"

            # Switch data channel back to clear text
            assert ftps.prot_c().startswith("200")
            clear_lines = []
            ftps.retrlines("RETR hello.txt", clear_lines.append)
            assert clear_lines == ["Hello FTP world!", "Second line"]

            # Switch control channel back to clear text via CCC
            assert ftps.ccc().startswith("200")
            assert isinstance(ftps.sock, socket.socket)
            assert ftps.voidcmd("NOOP").startswith("200")
            assert ftps.sendcmd("SHUTDOWN_SERVER").startswith("221")
            ftps.close()

        print("ftplib_tls_ok")
    finally:
        try:
            ftp_proc.wait(timeout=10)
        except Exception:
            ftp_proc.kill()
            ftp_proc.wait(timeout=5)
        if ftp_proc.stdout:
            ftp_proc.stdout.close()
        if ftp_proc.stderr:
            ftp_proc.stderr.close()

    assert ftp_proc.returncode == 0

    # --- Run SMTP / STARTTLS / SMTP_SSL / LMTP parity tests ---
    smtp_proc = subprocess.Popen(
        [sys.executable, "-c", _SMTP_SERVER_CODE, cert_file, key_file],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        ready = smtp_proc.stdout.readline().decode("ascii").strip()
        assert ready.startswith("READY "), f"unexpected SMTP server line: {ready!r}"
        _, p_plain, p_tls = ready.split()
        smtp_port = int(p_plain)
        smtp_ssl_port = int(p_tls)

        # 4a. Plain SMTP + EHLO + AUTH (CRAM-MD5, PLAIN, LOGIN) + sendmail + send_message
        with smtplib.SMTP("127.0.0.1", smtp_port, timeout=5) as s:
            code, _ = s.ehlo("client.local")
            assert code == 250
            assert s.does_esmtp is True
            assert s.has_extn("size")
            assert s.has_extn("starttls")
            assert s.has_extn("auth")
            assert b"help" in s.help().lower()
            assert s.noop()[0] == 250
            assert s.rset()[0] == 250
            assert s.verify("Alice <alice@example.com>")[0] == 250
            assert s.vrfy("alice@example.com")[0] == 250
            assert s.expn("list@example.com")[0] == 250

            # Login uses CRAM-MD5 first (preferred order)
            assert s.login("alice", "secret")[0] == 235
            # Explicit PLAIN and LOGIN auth calls
            s.user, s.password = "alice", "secret"
            assert s.auth("PLAIN", s.auth_plain)[0] == 235
            assert s.auth("LOGIN", s.auth_login)[0] == 235
            try:
                s.login("alice", "wrong")
                assert False
            except smtplib.SMTPAuthenticationError as err:
                assert err.smtp_code == 535

            # sendmail with dot-stuffing and partial recipient refusal
            errs = s.sendmail(
                "alice@example.com",
                ["ok@example.com", "bad@example.com"],
                "Subject: Test\n\nHello\n.dotline\nend",
            )
            assert list(errs.keys()) == ["bad@example.com"]
            assert errs["bad@example.com"][0] == 550

            # Verify server received unstuffed dotline
            import base64
            _, b64_last = s.docmd("XLAST")
            assert b"\r\n.dotline\r\n" in base64.b64decode(b64_last)

            # All recipients refused -> SMTPRecipientsRefused
            try:
                s.sendmail("alice@example.com", ["bad@example.com"], "Hello")
                assert False
            except smtplib.SMTPRecipientsRefused as err:
                assert "bad@example.com" in err.recipients

            # Sender refused -> SMTPSenderRefused
            try:
                s.sendmail("refused@example.com", ["ok@example.com"], "Hello")
                assert False
            except smtplib.SMTPSenderRefused as err:
                assert err.smtp_code == 550 and err.sender == "refused@example.com"

            # Data error -> SMTPDataError
            try:
                s.sendmail("alice@example.com", ["ok@example.com"], "TRIGGER_DATA_ERR")
                assert False
            except smtplib.SMTPDataError as err:
                assert err.smtp_code == 554

            # send_message with EmailMessage (strips Bcc header)
            em = email.message.EmailMessage()
            em["From"] = "Alice <alice@example.com>"
            em["To"] = "ok@example.com"
            em["Bcc"] = "secret@example.com"
            em["Subject"] = "Multipart test"
            em.set_content("Hello via send_message")
            assert s.send_message(em) == {}

        print("smtplib_plain_ok")

        # 4b. STARTTLS upgrade
        with smtplib.SMTP("127.0.0.1", smtp_port, timeout=5) as s:
            s.ehlo("client.local")
            assert s.has_extn("starttls")
            code, _ = s.starttls(context=cli_ctx)
            assert code == 220
            assert isinstance(s.sock, ssl.SSLSocket)
            assert s.ehlo_resp is None and s.does_esmtp is False
            s.ehlo("client.local")
            assert not s.has_extn("starttls")
            assert s.sendmail("alice@example.com", ["ok@example.com"], "Encrypted STARTTLS body") == {}

        print("smtplib_starttls_ok")

        # 4c. SMTP_SSL implicit TLS
        with smtplib.SMTP_SSL("127.0.0.1", smtp_ssl_port, context=cli_ctx, timeout=5) as s:
            assert isinstance(s.sock, ssl.SSLSocket)
            assert s.ehlo("client.local")[0] == 250
            assert s.sendmail("alice@example.com", ["ok@example.com"], "Implicit TLS body") == {}

        print("smtplib_ssl_ok")

        # 4d. LMTP (uses LHLO) + clean server shutdown
        with smtplib.LMTP("127.0.0.1", smtp_port, timeout=5) as lmtp:
            assert lmtp.ehlo_msg == "lhlo"
            assert lmtp.ehlo("lmtp.local")[0] == 250
            assert lmtp.sendmail("alice@example.com", ["ok@example.com"], "LMTP body") == {}
            assert lmtp.docmd("XSHUTDOWN")[0] == 221
            lmtp.close()

        print("smtplib_lmtp_ok")
    finally:
        try:
            smtp_proc.wait(timeout=10)
        except Exception:
            smtp_proc.kill()
            smtp_proc.wait(timeout=5)
        if smtp_proc.stdout:
            smtp_proc.stdout.close()
        if smtp_proc.stderr:
            smtp_proc.stderr.close()

    assert smtp_proc.returncode == 0

print("ftplib_smtplib_all_ok")
