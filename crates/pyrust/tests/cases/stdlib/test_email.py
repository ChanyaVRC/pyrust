"""CPython 3.12 parity tests for the `email` package (issue #3211)."""

import base64
import datetime
import inspect
import io
import sys
import email
import email.utils
import email.header
import email.policy
import email.message
import email.parser


# 1. Submodule identity & top-level exports
assert email.utils is sys.modules["email.utils"]
assert email.header is sys.modules["email.header"]
assert email.policy is sys.modules["email.policy"]
assert email.message is sys.modules["email.message"]
assert email.parser is sys.modules["email.parser"]
print("submodules_ok", True)

# 2. email.utils — address parsing & formatting
assert email.utils.parseaddr("Alice <alice@example.com>") == ("Alice", "alice@example.com")
assert email.utils.parseaddr('"Smith, Alice" <alice@example.com>') == ("Smith, Alice", "alice@example.com")
assert email.utils.parseaddr("bob@example.com") == ("", "bob@example.com")
assert email.utils.parseaddr("bob@example.com (Bob Jones)") == ("Bob Jones", "bob@example.com")
assert email.utils.parseaddr("") == ("", "")
if "strict" in inspect.signature(email.utils.parseaddr).parameters:
    assert email.utils.parseaddr("Alice <alice@example.com>", strict=True) == ("Alice", "alice@example.com")
    assert email.utils.parseaddr("Alice <alice@example.com>", strict=False) == ("Alice", "alice@example.com")
    assert email.utils.getaddresses(["Bob <bob@example.com>"], strict=False) == [("Bob", "bob@example.com")]

assert email.utils.formataddr(("Alice", "alice@example.com")) == "Alice <alice@example.com>"
assert email.utils.formataddr(("Smith, Alice", "alice@example.com")) == '"Smith, Alice" <alice@example.com>'
assert email.utils.formataddr(('A "B" \\C', "a@example.com")) == '"A \\"B\\" \\\\C" <a@example.com>'
assert email.utils.formataddr(("", "plain@example.com")) == "plain@example.com"
assert email.utils.formataddr(("José", "jose@example.com")) == "=?utf-8?b?Sm9zw6k=?= <jose@example.com>"

addrs = email.utils.getaddresses([
    "Alice <alice@example.com>, Bob <bob@example.com>",
    "Carol <carol@example.com>",
    "Team: dave@example.com, Eve <eve@example.com>;",
])
print("getaddresses", addrs)

# 3. email.utils — quote / unquote / make_msgid
assert email.utils.quote('a"b\\c') == 'a\\"b\\\\c'
assert email.utils.unquote('"hello \\"world\\" \\\\!"') == 'hello "world" \\!'
assert email.utils.unquote("<id@example.com>") == "id@example.com"
assert email.utils.unquote("plain") == "plain"

mid = email.utils.make_msgid(idstring="test", domain="example.org")
assert mid.startswith("<") and mid.endswith(".test@example.org>")
mid2 = email.utils.make_msgid()
assert mid2.startswith("<") and mid2.endswith(">") and "@" in mid2
print("msgid_ok", True)

# 4. email.utils — RFC 2822 dates
dstr = "Fri, 09 Nov 2001 01:08:47 -0000"
print("parsedate", email.utils.parsedate(dstr))
print("parsedate_tz", email.utils.parsedate_tz(dstr))
dt_naive = email.utils.parsedate_to_datetime(dstr)
print("parsedate_to_datetime_naive", dt_naive, dt_naive.tzinfo)

dstr_tz = "Tue, 15 Nov 1994 08:12:31 +0200"
ptz = email.utils.parsedate_tz(dstr_tz)
print("parsedate_tz_offset", ptz)
dt_aware = email.utils.parsedate_to_datetime(dstr_tz)
print("parsedate_to_datetime_aware", dt_aware.isoformat(), dt_aware.utcoffset())
ts = email.utils.mktime_tz(ptz)
print("mktime_tz", ts)
assert email.utils.format_datetime(dt_aware) == dstr_tz
assert email.utils.format_datetime(dt_naive) == dstr

dt_utc = datetime.datetime(2024, 1, 2, 3, 4, 5, tzinfo=datetime.timezone.utc)
print("format_datetime_utc", email.utils.format_datetime(dt_utc))
print("format_datetime_gmt", email.utils.format_datetime(dt_utc, usegmt=True))
print("formatdate_0", email.utils.formatdate(0))
print("formatdate_0_gmt", email.utils.formatdate(0, usegmt=True))
try:
    email.utils.format_datetime(dt_aware, usegmt=True)
    assert False
except ValueError as e:
    print("usegmt_non_utc_err", str(e))

assert email.utils.parsedate("not a date") is None
assert email.utils.parsedate_tz("") is None
try:
    email.utils.parsedate_to_datetime("invalid")
    assert False
except ValueError:
    print("bad_date_err", True)

lt = email.utils.localtime(dt_utc)
assert lt.tzinfo is not None
assert int(lt.timestamp()) == int(dt_utc.timestamp())
local_rfc = email.utils.formatdate(1700000000, localtime=True)
assert email.utils.mktime_tz(email.utils.parsedate_tz(local_rfc)) == 1700000000
print("localtime_ok", True)

# 5. email.utils — RFC 2231
enc_2231 = email.utils.encode_rfc2231("hello world", "utf-8", "en")
print("encode_rfc2231", enc_2231)
print("decode_rfc2231", email.utils.decode_rfc2231(enc_2231))
print("decode_rfc2231_plain", email.utils.decode_rfc2231("plain"))
assert email.utils.encode_rfc2231("foo bar") == "foo%20bar"
assert email.utils.encode_rfc2231("café", "utf-8") == "utf-8''caf%C3%A9"

params = email.utils.decode_params([
    ("attachment", ""),
    ("filename*0*", "utf-8''caf%C3%A9"),
    ("filename*1*", "%20menu.txt"),
])
print("decode_params", params)
print("collapse_rfc2231", email.utils.collapse_rfc2231_value(("utf-8", "", "caf\xc3\xa9")))
print("collapse_rfc2231_plain", email.utils.collapse_rfc2231_value('"quoted"'))

# 6. email.header — Header, decode_header, make_header
h = email.header.Header("Hello ", "us-ascii")
h.append("世界", "utf-8")
enc_h = h.encode()
print("header_encode", enc_h)
print("header_str", str(h))
assert h == "Hello 世界"

dec_b = email.header.decode_header("=?utf-8?B?SGVsbG8g5LiW55WM?=")
print("decode_b", dec_b)
dec_q = email.header.decode_header("=?utf-8?Q?Caf=C3=A9_au_lait?=")
print("decode_q", dec_q)
dec_mixed = email.header.decode_header("Re: =?iso-8859-1?q?p=F6stal?=  =?iso-8859-1?q?_mail?=")
print("decode_mixed", dec_mixed)
dec_plain = email.header.decode_header("Plain ASCII Subject")
print("decode_plain", dec_plain)
rebuilt = email.header.make_header(dec_mixed)
print("make_header", str(rebuilt))
print("decode_from_Header", email.header.decode_header(h))

# Base64 missing padding tolerance
dec_nopad = email.header.decode_header("=?utf-8?b?SGVsbG8?=")
print("decode_nopad", dec_nopad)

# Header folding & bytes input
h_bytes = email.header.Header(b"Caf\xc3\xa9", "utf-8")
assert str(h_bytes) == "Café"
h_fold = email.header.Header(
    "This is a fairly long ASCII subject line that should wrap nicely across lines",
    maxlinelen=40,
    header_name="Subject",
)
folded = h_fold.encode(linesep="\r\n")
assert "\r\n" in folded
print("header_fold_lines", len(folded.split("\r\n")))

# 7. email.policy
assert email.policy.compat32.linesep == "\n"
assert email.policy.compat32.mangle_from_ is True
assert email.policy.default.linesep == "\n"
assert email.policy.default.utf8 is False
assert email.policy.default.raise_on_defect is False
assert email.policy.strict.raise_on_defect is True
assert email.policy.SMTP.linesep == "\r\n"
assert email.policy.SMTPUTF8.linesep == "\r\n"
assert email.policy.SMTPUTF8.utf8 is True
assert email.policy.HTTP.linesep == "\r\n"
assert email.policy.HTTP.max_line_length is None

p_cloned = email.policy.default.clone(max_line_length=60, linesep="\r\n")
assert p_cloned.max_line_length == 60 and p_cloned.linesep == "\r\n"
p_added = email.policy.default + email.policy.SMTP
assert p_added.linesep == "\r\n"
try:
    p_cloned.linesep = "\n"
    assert False
except AttributeError:
    print("policy_readonly_ok", True)

# 8. email.message.Message — headers, params, payloads, walk, serialization
msg = email.message.Message()
msg["From"] = "alice@example.com"
msg["To"] = "bob@example.com"
msg["X-Tag"] = "first"
msg["x-tag"] = "second"
msg["Subject"] = "Hello"

assert len(msg) == 5
assert "from" in msg and "FROM" in msg and "Missing" not in msg
assert msg["from"] == "alice@example.com"
assert msg["missing"] is None
assert msg.get("missing", "fallback") == "fallback"
assert msg.get_all("X-Tag") == ["first", "second"]
assert msg.get_all("missing", []) == []
assert msg.keys() == ["From", "To", "X-Tag", "x-tag", "Subject"]
assert list(msg) == ["From", "To", "X-Tag", "x-tag", "Subject"]

msg.replace_header("subject", "Updated")
assert msg["Subject"] == "Updated"
try:
    msg.replace_header("nonexistent", "x")
    assert False
except KeyError:
    print("replace_header_keyerror_ok", True)

del msg["X-Tag"]
assert "x-tag" not in msg and len(msg) == 3

msg.add_header("Content-Type", "text/plain", charset="utf-8")
msg.add_header("Content-Disposition", "attachment", filename="report.pdf")
assert msg.get_content_type() == "text/plain"
assert msg.get_content_maintype() == "text"
assert msg.get_content_subtype() == "plain"
assert msg.get_content_charset() == "utf-8"
assert msg.get_param("charset") == "utf-8"
assert msg.get_filename() == "report.pdf"
assert msg.get_content_disposition() == "attachment"

msg.set_param("format", "flowed")
assert msg.get_param("format") == "flowed"
msg.del_param("format")
assert msg.get_param("format") is None

msg.set_type("text/html")
assert msg.get_content_type() == "text/html"
assert msg.get_content_charset() == "utf-8"
assert msg["MIME-Version"] == "1.0"

msg.set_payload("Hello body\n")
assert not msg.is_multipart()
assert msg.get_payload() == "Hello body\n"
assert msg.get_charsets() == ["utf-8"]
print("msg_as_string", repr(msg.as_string()))
assert msg.as_bytes() == msg.as_string().encode("ascii")

# Base64 & Quoted-Printable payload decoding
b64_msg = email.message.Message()
b64_msg["Content-Type"] = "text/plain; charset=utf-8"
b64_msg["Content-Transfer-Encoding"] = "base64"
b64_msg.set_payload(base64.b64encode("Hello 世界!".encode("utf-8")).decode("ascii"))
print("b64_decoded", b64_msg.get_payload(decode=True).decode("utf-8"))

qp_msg = email.message.Message()
qp_msg["Content-Type"] = "text/plain; charset=utf-8"
qp_msg["Content-Transfer-Encoding"] = "quoted-printable"
qp_msg.set_payload("Caf=C3=A9=20line1=\nline2")
print("qp_decoded", qp_msg.get_payload(decode=True).decode("utf-8"))

# 9. email.parser & top-level helpers — unfolding, multipart, nested, headersonly
raw_email = (
    "From sender@example.com Mon Jan  1 00:00:00 2024\n"
    "From: Alice <alice@example.com>\n"
    "To: Bob <bob@example.com>\n"
    "Subject: Long subject\n"
    " that continues on next line\n"
    'Content-Type: multipart/mixed; boundary="outer"\n'
    "\n"
    "This is the preamble.\n"
    "--outer\n"
    'Content-Type: multipart/alternative; boundary="inner"\n'
    "\n"
    "--inner\n"
    "Content-Type: text/plain; charset=utf-8\n"
    "Content-Transfer-Encoding: quoted-printable\n"
    "\n"
    "Hello =E4=B8=96=E7=95=8C!\n"
    "--inner\n"
    "Content-Type: text/html; charset=utf-8\n"
    "Content-Transfer-Encoding: base64\n"
    "\n"
    "PGI+SGVsbG88L2I+\n"
    "--inner--\n"
    "--outer\n"
    'Content-Type: application/octet-stream; name="data.bin"\n'
    'Content-Disposition: attachment; filename="data.bin"\n'
    "Content-Transfer-Encoding: base64\n"
    "\n"
    "AQIDBA==\n"
    "--outer--\n"
    "Epilogue text"
)

parsed = email.message_from_string(raw_email)
print("unixfrom", parsed.get_unixfrom())
print("subject_compat32", repr(parsed["Subject"]))
print("preamble", repr(parsed.preamble))
print("epilogue", repr(parsed.epilogue))
print("boundary", parsed.get_boundary())
print("walk_types", [p.get_content_type() for p in parsed.walk()])
print("charsets", parsed.get_charsets())

outer_parts = parsed.get_payload()
assert len(outer_parts) == 2
assert parsed.get_payload(decode=True) is None
inner = outer_parts[0]
assert inner.is_multipart()
assert inner.get_payload(0).get_payload(decode=True).decode("utf-8") == "Hello 世界!"
assert inner.get_payload(1).get_payload(decode=True) == b"<b>Hello</b>"
attach = outer_parts[1]
assert attach.get_filename() == "data.bin"
assert attach.get_payload(decode=True) == b"\x01\x02\x03\x04"

# Roundtrip via as_string -> message_from_string
reparsed = email.message_from_string(parsed.as_string())
assert [p.get_content_type() for p in reparsed.walk()] == [
    p.get_content_type() for p in parsed.walk()
]
assert reparsed.get_payload(1).get_payload(decode=True) == b"\x01\x02\x03\x04"
print("multipart_roundtrip_ok", True)

# BytesParser / message_from_bytes / file parsers / HeaderParser
parsed_bytes = email.message_from_bytes(raw_email.encode("utf-8"))
assert [p.get_content_type() for p in parsed_bytes.walk()] == [
    p.get_content_type() for p in parsed.walk()
]
parsed_file = email.message_from_file(io.StringIO(raw_email))
assert parsed_file.get_content_type() == "multipart/mixed"
parsed_bfile = email.message_from_binary_file(io.BytesIO(raw_email.encode("utf-8")))
assert parsed_bfile.get_content_type() == "multipart/mixed"

hdr_only = email.parser.HeaderParser().parsestr(raw_email)
assert not hdr_only.is_multipart()
assert isinstance(hdr_only.get_payload(), str)
assert "--outer" in hdr_only.get_payload()

bhdr_only = email.parser.BytesHeaderParser().parsebytes(raw_email.encode("utf-8"))
assert not bhdr_only.is_multipart()
assert isinstance(bhdr_only.get_payload(), str)
print("header_parsers_ok", True)

# FeedParser & BytesFeedParser
fp = email.parser.FeedParser()
fp.feed("Subject: Chunked\n\n")
fp.feed("Body line 1\n")
fp.feed("Body line 2")
fmsg = fp.close()
assert fmsg["Subject"] == "Chunked" and fmsg.get_payload() == "Body line 1\nBody line 2"

bfp = email.parser.BytesFeedParser()
bfp.feed(b"Subject: BytesChunked\r\n\r\nData")
bfmsg = bfp.close()
assert bfmsg["Subject"] == "BytesChunked" and bfmsg.get_payload() == "Data"
print("feed_parsers_ok", True)

# 10. EmailMessage & policy.default
em = email.message.EmailMessage()
em["From"] = "alice@example.com"
em["To"] = "bob@example.com"
em["Subject"] = "Greetings"
em.set_content("Plain text body")
assert em["MIME-Version"] == "1.0"
assert em.get_content() == "Plain text body\n"

em.add_alternative("<p>HTML body</p>", subtype="html")
em.add_attachment(
    b"\x89PNG\r\n",
    maintype="image",
    subtype="png",
    filename="pic.png",
)
assert em.get_content_type() == "multipart/mixed"
assert em.get_body().get_content() == "<p>HTML body</p>\n"
assert em.get_body(preferencelist=("plain",)).get_content() == "Plain text body\n"
atts = list(em.iter_attachments())
assert len(atts) == 1
assert atts[0].is_attachment()
assert atts[0].get_filename() == "pic.png"
assert atts[0].get_content() == b"\x89PNG\r\n"

# Parse EmailMessage back with policy.default
em_parsed = email.message_from_bytes(em.as_bytes(), policy=email.policy.default)
assert isinstance(em_parsed, email.message.EmailMessage)
assert em_parsed["Subject"] == "Greetings"
assert em_parsed.get_body().get_content() == "<p>HTML body</p>\n"
assert list(em_parsed.iter_attachments())[0].get_content() == b"\x89PNG\r\n"

# RFC 2047 header decoding with policy.default
m_utf8 = email.message_from_string(
    "Subject: =?utf-8?b?SGVsbG8g5LiW55WM?=\n\nBody\n",
    policy=email.policy.default,
)
assert str(m_utf8["Subject"]) == "Hello 世界"

# Duplicate single-occurrence header error on EmailMessage
try:
    em["Subject"] = "Second subject"
    assert False
except ValueError:
    print("email_message_max_count_ok", True)

# CR/LF rejection in EmailPolicy header value
em2 = email.message.EmailMessage()
try:
    em2["Subject"] = "line1\nline2"
    assert False
except ValueError:
    print("email_message_crlf_reject_ok", True)

# Multipart constructed from scratch with auto-generated boundary, set_boundary,
# set_payload(charset=...), multipart/digest, and EmailMessage clear/clear_content
mp = email.message.Message()
mp["Content-Type"] = "multipart/mixed"
p1 = email.message.Message()
p1.set_payload("Café", charset="utf-8")
p2 = email.message.Message()
p2.set_payload("Plain part")
mp.attach(p1)
mp.attach(p2)
assert mp.is_multipart()
serialized = mp.as_string()
assert mp.get_boundary() is not None
reparsed_mp = email.message_from_string(serialized)
assert len(reparsed_mp.get_payload()) == 2
assert reparsed_mp.get_payload(0).get_payload(decode=True).decode("utf-8") == "Café"
assert reparsed_mp.get_payload(1).get_payload() == "Plain part"

mp.set_boundary("custom-boundary-123")
assert mp.get_boundary() == "custom-boundary-123"
assert mp.get_default_type() == "text/plain"
mp.set_default_type("message/rfc822")
assert mp.get_default_type() == "message/rfc822"

digest_raw = (
    'Content-Type: multipart/digest; boundary="dig"\n'
    "\n"
    "--dig\n"
    "\n"
    "From: inner@example.com\n"
    "Subject: Inner\n"
    "\n"
    "Digest item body\n"
    "--dig--\n"
)
dig = email.message_from_string(digest_raw)
assert dig.get_payload(0).get_content_type() == "message/rfc822"
assert dig.get_payload(0).get_payload(0)["Subject"] == "Inner"

em3 = email.message.EmailMessage()
em3["Subject"] = "ClearTest"
em3.set_content("Some body")
em3.clear_content()
assert em3["Subject"] == "ClearTest" and em3.get_payload() is None
em3.clear()
assert len(em3) == 0 and em3.get_payload() is None
print("extra_message_cases_ok", True)

print("ALL EMAIL TESTS PASSED")
