"""CPython 3.12 parity tests for email.errors, base64mime, quoprimime, encoders, charset, and email.mime.* (issue #3257)."""

import sys
import email
import email.errors
import email.base64mime
import email.quoprimime
import email.encoders
import email.charset
import email.message
import email.mime
import email.mime.base
import email.mime.nonmultipart
import email.mime.multipart
import email.mime.text
import email.mime.application
import email.mime.audio
import email.mime.image
import email.mime.message

# 1. Submodule identity & __all__
assert email.errors is sys.modules["email.errors"]
assert email.base64mime is sys.modules["email.base64mime"]
assert email.quoprimime is sys.modules["email.quoprimime"]
assert email.encoders is sys.modules["email.encoders"]
assert email.charset is sys.modules["email.charset"]
assert email.mime is sys.modules["email.mime"]
assert email.mime.text is sys.modules["email.mime.text"]
print("email_all", sorted(email.__all__))
print("encoders_all", email.encoders.__all__)
print("base64mime_all", email.base64mime.__all__)
print("quoprimime_all", email.quoprimime.__all__)
print("charset_all", email.charset.__all__)

# 2. email.errors hierarchy
assert issubclass(email.errors.MessageError, Exception)
assert issubclass(email.errors.MessageParseError, email.errors.MessageError)
assert issubclass(email.errors.HeaderParseError, email.errors.MessageParseError)
assert issubclass(email.errors.BoundaryError, email.errors.MessageParseError)
assert issubclass(email.errors.MultipartConversionError, email.errors.MessageError)
assert issubclass(email.errors.MultipartConversionError, TypeError)
assert issubclass(email.errors.CharsetError, email.errors.MessageError)
assert issubclass(email.errors.MessageDefect, ValueError)
assert email.errors.MalformedHeaderDefect is email.errors.MissingHeaderBodySeparatorDefect

for defect_cls in [
    email.errors.NoBoundaryInMultipartDefect,
    email.errors.StartBoundaryNotFoundDefect,
    email.errors.CloseBoundaryNotFoundDefect,
    email.errors.FirstHeaderLineIsContinuationDefect,
    email.errors.MisplacedEnvelopeHeaderDefect,
    email.errors.MissingHeaderBodySeparatorDefect,
    email.errors.MultipartInvariantViolationDefect,
    email.errors.InvalidMultipartContentTransferEncodingDefect,
    email.errors.UndecodableBytesDefect,
    email.errors.InvalidBase64PaddingDefect,
    email.errors.InvalidBase64CharactersDefect,
    email.errors.InvalidBase64LengthDefect,
    email.errors.HeaderDefect,
]:
    assert issubclass(defect_cls, email.errors.MessageDefect)

for hdefect_cls in [
    email.errors.InvalidHeaderDefect,
    email.errors.HeaderMissingRequiredValue,
    email.errors.NonPrintableDefect,
    email.errors.ObsoleteHeaderDefect,
    email.errors.NonASCIILocalPartDefect,
    email.errors.InvalidDateDefect,
]:
    assert issubclass(hdefect_cls, email.errors.HeaderDefect)

d1 = email.errors.MessageDefect("bad line")
assert d1.line == "bad line" and str(d1) == "bad line"
d2 = email.errors.MessageDefect()
assert d2.line is None
npd = email.errors.NonPrintableDefect(["\x00", "\x07"])
assert npd.non_printables == ["\x00", "\x07"]
print("non_printable_str", str(npd))
print("errors_ok", True)

# 3. email.base64mime
assert email.base64mime.CRLF == "\r\n"
assert email.base64mime.NL == "\n"
assert email.base64mime.EMPTYSTRING == ""
assert email.base64mime.MISC_LEN == 7
print("b64_hlen", [email.base64mime.header_length(b"a" * i) for i in range(7)])
print("b64_henc_empty", repr(email.base64mime.header_encode(b"")))
print("b64_henc_bytes", email.base64mime.header_encode(b"hello world", "utf-8"))
print("b64_henc_str", email.base64mime.header_encode("hello", "iso-8859-1"))
print("b64_benc_empty", repr(email.base64mime.body_encode(b"")))
print("b64_benc_wrap", repr(email.base64mime.body_encode(b"0123456789abcdef", maxlinelen=12, eol="\r\n")))
print("b64_dec_str", email.base64mime.decode("aGVsbG8gd29ybGQ="))
print("b64_dec_bytes", email.base64mime.body_decode(b"aGVsbG8="))
print("b64_dec_empty", email.base64mime.decodestring(""))

# 4. email.quoprimime
assert email.quoprimime.CRLF == "\r\n"
assert email.quoprimime.NL == "\n"
assert email.quoprimime.EMPTYSTRING == ""
print("qp_hcheck", email.quoprimime.header_check(ord("a")), email.quoprimime.header_check(ord(" ")), email.quoprimime.header_check(ord("=")))
print("qp_bcheck", email.quoprimime.body_check(ord("a")), email.quoprimime.body_check(ord(" ")), email.quoprimime.body_check(ord("=")), email.quoprimime.body_check(ord("\t")))
print("qp_hlen", email.quoprimime.header_length(b"hello world=1"))
print("qp_blen", email.quoprimime.body_length(b"hello world=1"))
print("qp_quote_unquote", email.quoprimime.quote("="), email.quoprimime.unquote("=3D"), email.quoprimime.quote(" "))
print("qp_henc", email.quoprimime.header_encode(b"hello world =?", "utf-8"))
print("qp_henc_empty", repr(email.quoprimime.header_encode(b"")))
print("qp_hdec", email.quoprimime.header_decode("hello_world_=3D=3F"))
print("qp_benc_simple", repr(email.quoprimime.body_encode("Hello = world \nLine 2\n")))
print("qp_benc_wrap", repr(email.quoprimime.body_encode("abcdefghij=klmnop", maxlinelen=10, eol="\r\n")))
print("qp_bdec", repr(email.quoprimime.decode("Hello =3D world=20\nLine 2\n")))
print("qp_bdec_soft", repr(email.quoprimime.body_decode("hello =\nworld")))
print("qp_decodestring", repr(email.quoprimime.decodestring("a=20b")))
try:
    email.quoprimime.body_encode("abc", maxlinelen=3)
    assert False
except ValueError as e:
    print("qp_minlen_err", str(e))

# 5. email.encoders
m_b64 = email.message.Message()
m_b64.set_payload(b"binary \x00\xff data")
email.encoders.encode_base64(m_b64)
print("enc_b64", m_b64["Content-Transfer-Encoding"], repr(m_b64.get_payload()), m_b64.get_payload(decode=True))

m_qp = email.message.Message()
m_qp.set_payload(b"hello world = \xff")
email.encoders.encode_quopri(m_qp)
print("enc_qp", m_qp["Content-Transfer-Encoding"], repr(m_qp.get_payload()))

m_7 = email.message.Message()
m_7.set_payload("pure ascii")
email.encoders.encode_7or8bit(m_7)
print("enc_7", m_7["Content-Transfer-Encoding"])

m_8 = email.message.Message()
m_8.set_payload(b"non-ascii \xc3\xa9")
email.encoders.encode_7or8bit(m_8)
print("enc_8", m_8["Content-Transfer-Encoding"])

m_none = email.message.Message()
email.encoders.encode_7or8bit(m_none)
print("enc_none", m_none["Content-Transfer-Encoding"])

m_noop = email.message.Message()
m_noop.set_payload("untouched")
email.encoders.encode_noop(m_noop)
assert m_noop["Content-Transfer-Encoding"] is None and m_noop.get_payload() == "untouched"

# 6. email.charset
assert email.charset.QP == 1
assert email.charset.BASE64 == 2
assert email.charset.SHORTEST == 3
cs_ascii = email.charset.Charset("ascii")
assert cs_ascii.input_charset == "us-ascii"
assert cs_ascii.output_charset == "us-ascii"
assert cs_ascii.get_body_encoding() is email.encoders.encode_7or8bit
assert str(cs_ascii) == "us-ascii" and repr(cs_ascii) == "us-ascii"
assert cs_ascii == "US-ASCII"

cs_l1 = email.charset.Charset("latin-1")
assert cs_l1.input_charset == "iso-8859-1"
assert cs_l1.header_encoding == email.charset.QP
assert cs_l1.body_encoding == email.charset.QP
assert cs_l1.get_body_encoding() == "quoted-printable"
print("cs_l1_henc", cs_l1.header_encode("caf\xe9"))
print("cs_l1_benc", repr(cs_l1.body_encode("caf\xe9")))

cs_u8 = email.charset.Charset("utf-8")
assert cs_u8.get_body_encoding() == "base64"
print("cs_u8_henc_short", cs_u8.header_encode("caf\xe9"))
print("cs_u8_henc_cjk", cs_u8.header_encode("你好世界"))
print("cs_u8_benc", repr(cs_u8.body_encode("你好")))

email.charset.add_charset("x-custom-cs", email.charset.QP, email.charset.BASE64, "utf-8")
email.charset.add_alias("x-custom-alias", "x-custom-cs")
email.charset.add_codec("x-custom-cs", "utf-8")
cs_custom = email.charset.Charset("x-custom-alias")
assert cs_custom.input_charset == "x-custom-cs"
assert cs_custom.output_charset == "utf-8"
assert cs_custom.header_encoding == email.charset.QP
assert cs_custom.body_encoding == email.charset.BASE64

try:
    email.charset.add_charset("bad", email.charset.QP, email.charset.SHORTEST)
    assert False
except ValueError:
    print("cs_shortest_body_err", True)

try:
    email.charset.Charset("caf\xe9")
    assert False
except email.errors.CharsetError:
    print("cs_non_ascii_err", True)

# 7. email.mime.* classes
mb = email.mime.base.MIMEBase("application", "x-test", name="demo.bin")
assert isinstance(mb, email.message.Message)
assert mb["MIME-Version"] == "1.0"
assert mb.get_content_type() == "application/x-test"
assert mb.get_param("name") == "demo.bin"

nm = email.mime.nonmultipart.MIMENonMultipart("text", "plain")
assert isinstance(nm, email.mime.base.MIMEBase)
try:
    nm.attach(email.message.Message())
    assert False
except email.errors.MultipartConversionError:
    print("nm_attach_err", True)

mt_ascii = email.mime.text.MIMEText("Hello ASCII")
assert mt_ascii.get_content_type() == "text/plain"
assert mt_ascii.get_content_charset() == "us-ascii"
assert mt_ascii["Content-Transfer-Encoding"] == "7bit"
assert mt_ascii.get_payload() == "Hello ASCII"

mt_utf8 = email.mime.text.MIMEText("Hello 世界", "html")
assert mt_utf8.get_content_type() == "text/html"
assert mt_utf8.get_content_charset() == "utf-8"
assert mt_utf8["Content-Transfer-Encoding"] == "base64"
assert mt_utf8.get_payload(decode=True).decode("utf-8") == "Hello 世界"

ma = email.mime.application.MIMEApplication(b"\x01\x02\x03\x04", "pdf", name="doc.pdf")
assert ma.get_content_type() == "application/pdf"
assert ma.get_param("name") == "doc.pdf"
assert ma.get_payload(decode=True) == b"\x01\x02\x03\x04"
try:
    email.mime.application.MIMEApplication(b"abc", _subtype=None)
    assert False
except TypeError:
    print("mime_app_none_err", True)

# Audio sniffing
wav_hdr = b"RIFF\x24\x00\x00\x00WAVEfmt \x10\x00\x00\x00"
aiff_hdr = b"FORM\x00\x00\x00\x10AIFFdata"
au_hdr = b".snd\x00\x00\x00\x18"
assert email.mime.audio.MIMEAudio(wav_hdr).get_content_type() == "audio/x-wav"
assert email.mime.audio.MIMEAudio(aiff_hdr).get_content_type() == "audio/x-aiff"
assert email.mime.audio.MIMEAudio(au_hdr).get_content_type() == "audio/basic"
assert email.mime.audio.MIMEAudio(b"raw", "mpeg").get_content_type() == "audio/mpeg"
try:
    email.mime.audio.MIMEAudio(b"unknown bytes")
    assert False
except TypeError:
    print("mime_audio_unknown_err", True)

# Image sniffing
png_hdr = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"
jpg_hdr = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01"
gif_hdr = b"GIF89a\x01\x00\x01\x00"
bmp_hdr = b"BM\x36\x00\x00\x00"
webp_hdr = b"RIFF\x10\x00\x00\x00WEBP"
assert email.mime.image.MIMEImage(png_hdr).get_content_type() == "image/png"
assert email.mime.image.MIMEImage(jpg_hdr).get_content_type() == "image/jpeg"
assert email.mime.image.MIMEImage(gif_hdr).get_content_type() == "image/gif"
assert email.mime.image.MIMEImage(bmp_hdr).get_content_type() == "image/bmp"
assert email.mime.image.MIMEImage(webp_hdr).get_content_type() == "image/webp"
assert email.mime.image.MIMEImage(b"raw", "svg+xml").get_content_type() == "image/svg+xml"
try:
    email.mime.image.MIMEImage(b"not an image")
    assert False
except TypeError:
    print("mime_image_unknown_err", True)

# MIMEMessage & MIMEMultipart
inner_msg = email.mime.text.MIMEText("Wrapped message")
inner_msg["Subject"] = "Inner"
mm = email.mime.message.MIMEMessage(inner_msg)
assert mm.get_content_type() == "message/rfc822"
assert mm.get_default_type() == "message/rfc822"
assert mm.get_payload(0)["Subject"] == "Inner"
try:
    email.mime.message.MIMEMessage("not a message")
    assert False
except TypeError:
    print("mime_msg_type_err", True)

mp = email.mime.multipart.MIMEMultipart("alternative", boundary="my-bound", _subparts=[mt_ascii, mt_utf8])
mp["From"] = "alice@example.com"
mp["To"] = "bob@example.com"
mp["Subject"] = "Multipart Test"
assert mp.is_multipart()
assert mp.get_content_type() == "multipart/alternative"
assert mp.get_boundary() == "my-bound"
assert len(mp.get_payload()) == 2
reparsed = email.message_from_string(mp.as_string())
assert reparsed.get_content_type() == "multipart/alternative"
assert len(reparsed.get_payload()) == 2
assert reparsed.get_payload(0).get_payload() == "Hello ASCII"
assert reparsed.get_payload(1).get_payload(decode=True).decode("utf-8") == "Hello 世界"
print("ALL MIME ENCODERS TESTS PASSED")
