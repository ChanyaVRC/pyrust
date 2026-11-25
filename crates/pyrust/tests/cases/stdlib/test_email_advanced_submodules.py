"""CPython 3.12 parity tests for email.iterators, feedparser, generator, contentmanager, headerregistry (issue #3257)."""

import datetime
import io
import sys
import types
import email
import email.errors
import email.iterators
import email.feedparser
import email.parser
import email.generator
import email.contentmanager
import email.headerregistry
import email.policy
import email.message
from email.mime.multipart import MIMEMultipart
from email.mime.text import MIMEText
from email.mime.application import MIMEApplication

# 1. Submodule & object identity
assert email.iterators is sys.modules["email.iterators"]
assert email.feedparser is sys.modules["email.feedparser"]
assert email.generator is sys.modules["email.generator"]
assert email.contentmanager is sys.modules["email.contentmanager"]
assert email.headerregistry is sys.modules["email.headerregistry"]
assert email.parser.FeedParser is email.feedparser.FeedParser
assert email.parser.BytesFeedParser is email.feedparser.BytesFeedParser
assert email.policy.default.content_manager is email.contentmanager.raw_data_manager
print("identity_ok", True)

# 2. email.iterators
outer = MIMEMultipart("mixed", boundary="outer")
alt = MIMEMultipart("alternative", boundary="inner")
alt.attach(MIMEText("Plain line 1\nPlain line 2\n", "plain"))
alt.attach(MIMEText("<p>HTML</p>\n", "html"))
outer.attach(alt)
att = MIMEApplication(b"\x01\x02", "octet-stream")
att.add_header("Content-Disposition", "attachment", filename="f.bin")
outer.attach(att)

walked = list(email.iterators.walk(outer))
print("walk_types", [p.get_content_type() for p in walked])
lines = list(email.iterators.body_line_iterator(outer))
print("body_lines", lines)
text_parts = list(email.iterators.typed_subpart_iterator(outer, "text"))
print("text_parts", [p.get_content_type() for p in text_parts])
html_parts = list(email.iterators.typed_subpart_iterator(outer, "text", "html"))
print("html_parts", [p.get_content_type() for p in html_parts])

struct_buf = io.StringIO()
email.iterators._structure(outer, fp=struct_buf)
print("structure", repr(struct_buf.getvalue()))
struct_def = io.StringIO()
email.iterators._structure(outer, fp=struct_def, include_default=True)
print("structure_default", repr(struct_def.getvalue()))

# 3. email.feedparser & defects
fp = email.feedparser.FeedParser()
fp.feed("From: a@example.com\nSubject: Hi\n")
fp.feed("\nBody text\n")
m = fp.close()
assert m["From"] == "a@example.com" and m["Subject"] == "Hi" and m.get_payload() == "Body text\n"

bfp = email.feedparser.BytesFeedParser()
bfp.feed(b"Subject: Binary\r\n\r\nHello\r\n")
bm = bfp.close()
assert bm["Subject"] == "Binary" and bm.get_payload() == "Hello\r\n"

# Defects: missing boundary
bad_mp = email.message_from_string("Content-Type: multipart/mixed\n\nBody\n")
assert any(isinstance(d, email.errors.NoBoundaryInMultipartDefect) for d in bad_mp.defects)

# Defects: start boundary not found
bad_start = email.message_from_string('Content-Type: multipart/mixed; boundary="abc"\n\nNo boundary here\n')
assert any(isinstance(d, email.errors.StartBoundaryNotFoundDefect) for d in bad_start.defects)

# Defects: close boundary not found
bad_close = email.message_from_string('Content-Type: multipart/mixed; boundary="abc"\n\n--abc\n\nPart 1\n')
assert any(isinstance(d, email.errors.CloseBoundaryNotFoundDefect) for d in bad_close.defects)

# Defects: strict policy raises
try:
    email.message_from_string("Content-Type: multipart/mixed\n\nBody\n", policy=email.policy.strict)
    assert False
except email.errors.NoBoundaryInMultipartDefect:
    print("strict_defect_raised", True)

# 4. email.generator
g_msg = email.message.Message()
g_msg["From"] = "alice@example.com"
g_msg["Subject"] = "Generator Test"
g_msg.set_unixfrom("From alice@example.com Mon Jan  1 00:00:00 2024")
g_msg.set_payload("Line 1\nFrom someone in body\nLine 3\n")

s_buf = io.StringIO()
gen = email.generator.Generator(s_buf, mangle_from_=True)
gen.flatten(g_msg, unixfrom=True, linesep="\r\n")
print("gen_mangled", repr(s_buf.getvalue()))

s_buf2 = io.StringIO()
gen2 = email.generator.Generator(s_buf2, mangle_from_=False)
gen2.flatten(g_msg, unixfrom=False)
print("gen_unmangled", repr(s_buf2.getvalue()))

b_buf = io.BytesIO()
bgen = email.generator.BytesGenerator(b_buf, mangle_from_=False)
bgen.flatten(outer)
reparsed_b = email.message_from_bytes(b_buf.getvalue())
assert [p.get_content_type() for p in reparsed_b.walk()] == [p.get_content_type() for p in outer.walk()]
print("bgen_ok", True)

d_buf = io.StringIO()
dgen = email.generator.DecodedGenerator(d_buf)
dgen.flatten(outer)
print("dgen_out", repr(d_buf.getvalue()))

d_buf2 = io.StringIO()
dgen2 = email.generator.DecodedGenerator(d_buf2, fmt="OMITTED:%(type)s:%(filename)s")
dgen2.flatten(outer)
assert "OMITTED:application/octet-stream:f.bin" in d_buf2.getvalue()
print("dgen_custom_ok", True)

# 5. email.contentmanager
cm = email.contentmanager.ContentManager()
cm.add_get_handler("text/plain", lambda msg: "EXACT:" + msg.get_payload())
cm.add_get_handler("text", lambda msg: "MAIN:" + msg.get_payload())
cm.add_get_handler("", lambda msg: "FALLBACK:" + msg.get_payload())

m1 = email.message.EmailMessage()
m1.set_content("hello")
assert cm.get_content(m1) == "EXACT:hello\n"
m2 = email.message.EmailMessage()
m2.set_content("<b>hi</b>", subtype="html")
assert cm.get_content(m2) == "MAIN:<b>hi</b>\n"
m3 = email.message.EmailMessage()
m3.set_content(b"raw", maintype="application", subtype="octet-stream", cte="7bit")
assert cm.get_content(m3) == "FALLBACK:raw"

# raw_data_manager direct usage
em = email.message.EmailMessage()
email.contentmanager.raw_data_manager.set_content(em, "Text content", subtype="plain", charset="utf-8")
assert email.contentmanager.raw_data_manager.get_content(em) == "Text content\n"

em_b = email.message.EmailMessage()
email.contentmanager.raw_data_manager.set_content(
    em_b, bytearray(b"\xde\xad\xbe\xef"), maintype="application", subtype="octet-stream", filename="x.bin"
)
assert em_b.get_filename() == "x.bin"
assert email.contentmanager.raw_data_manager.get_content(em_b) == b"\xde\xad\xbe\xef"

em_m = email.message.EmailMessage()
email.contentmanager.raw_data_manager.set_content(em_m, em)
assert em_m.get_content_type() == "message/rfc822"
assert email.contentmanager.raw_data_manager.get_content(em_m).get_content() == "Text content\n"

try:
    email.contentmanager.raw_data_manager.set_content(outer, "fail")
    assert False
except TypeError:
    print("cm_multipart_err", True)

# 6. email.headerregistry
addr1 = email.headerregistry.Address("Alice Smith", "alice", "example.com")
assert addr1.display_name == "Alice Smith"
assert addr1.username == "alice"
assert addr1.domain == "example.com"
assert addr1.addr_spec == "alice@example.com"
print("addr1_str", str(addr1))
print("addr1_repr", repr(addr1))

addr2 = email.headerregistry.Address(display_name="Smith, Bob", addr_spec="bob@example.org")
assert addr2.username == "bob" and addr2.domain == "example.org"
print("addr2_str", str(addr2))
assert addr1 == email.headerregistry.Address("Alice Smith", "alice", "example.com")
assert addr1 != addr2

try:
    email.headerregistry.Address("Bad\nName", "a", "b.com")
    assert False
except ValueError:
    print("addr_crlf_err", True)

try:
    email.headerregistry.Address(username="a", domain="b.com", addr_spec="a@b.com")
    assert False
except TypeError:
    print("addr_both_err", True)

grp1 = email.headerregistry.Group("Team", [addr1, addr2])
print("grp1_str", str(grp1))
print("grp1_repr", repr(grp1))
grp_empty = email.headerregistry.Group("Undisclosed")
print("grp_empty_str", str(grp_empty))
grp_single = email.headerregistry.Group(None, [addr1])
assert str(grp_single) == str(addr1)

reg = email.headerregistry.HeaderRegistry()

h_subj = reg("Subject", "Hello World")
assert isinstance(h_subj, str) and isinstance(h_subj, email.headerregistry.BaseHeader)
assert h_subj.name == "Subject" and str(h_subj) == "Hello World" and h_subj.defects == ()
assert h_subj.max_count == 1
print("h_subj_fold", repr(h_subj.fold(policy=email.policy.default)))

h_date = reg("Date", "Fri, 09 Nov 2001 01:08:47 -0000")
assert h_date.datetime == datetime.datetime(2001, 11, 9, 1, 8, 47)
assert str(h_date) == "Fri, 09 Nov 2001 01:08:47 -0000"
h_date_dt = reg("Date", datetime.datetime(2024, 5, 6, 7, 8, 9, tzinfo=datetime.timezone.utc))
print("h_date_dt", str(h_date_dt))
h_date_bad = reg("Date", "not a date")
assert h_date_bad.datetime is None and len(h_date_bad.defects) == 1
assert isinstance(h_date_bad.defects[0], email.errors.InvalidDateDefect)

h_to = reg("To", "Alice <alice@example.com>, Team: bob@example.org, carol@example.org;")
assert len(h_to.addresses) == 3
assert len(h_to.groups) == 2
assert h_to.groups[1].display_name == "Team"
print("h_to_addrs", [a.addr_spec for a in h_to.addresses])

h_sender = reg("Sender", "Alice <alice@example.com>")
assert h_sender.address == email.headerregistry.Address("Alice", "alice", "example.com")
h_sender_bad = reg("Sender", "a@b.com, c@d.com")
try:
    _ = h_sender_bad.address
    assert False
except ValueError:
    print("single_addr_err", True)

h_mv = reg("MIME-Version", "1.0")
assert h_mv.major == 1 and h_mv.minor == 0 and h_mv.version == "1.0"

h_ct = reg("Content-Type", 'text/plain; charset="utf-8"; format=flowed')
assert h_ct.maintype == "text" and h_ct.subtype == "plain" and h_ct.content_type == "text/plain"
assert isinstance(h_ct.params, types.MappingProxyType)
print("h_ct_params", sorted(h_ct.params.items()))

h_cd = reg("Content-Disposition", 'attachment; filename="report.pdf"')
assert h_cd.content_disposition == "attachment"
assert h_cd.params["filename"] == "report.pdf"

h_cte = reg("Content-Transfer-Encoding", "Base64")
assert h_cte.cte == "base64"

h_mid = reg("Message-ID", "<abc123@example.com>")
assert str(h_mid) == "<abc123@example.com>" and h_mid.defects == ()

reg.map_to_type("X-Custom-Date", email.headerregistry.DateHeader)
h_cdate = reg("X-Custom-Date", "Fri, 09 Nov 2001 01:08:47 -0000")
assert h_cdate.datetime == datetime.datetime(2001, 11, 9, 1, 8, 47)

print("ALL ADVANCED EMAIL TESTS PASSED")
