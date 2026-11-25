# Comprehensive CPython 3.12 parity tests for `codecs` (issue #3203).

import codecs
import io
import os
import tempfile

# 1. BOM constants
for name in [
    "BOM",
    "BOM_UTF8",
    "BOM_LE",
    "BOM_BE",
    "BOM_UTF16",
    "BOM_UTF16_LE",
    "BOM_UTF16_BE",
    "BOM_UTF32",
    "BOM_UTF32_LE",
    "BOM_UTF32_BE",
    "BOM32_LE",
    "BOM32_BE",
    "BOM64_LE",
    "BOM64_BE",
]:
    print("bom", name, repr(getattr(codecs, name)))

# 2. CodecInfo structure & lookup
print("CodecInfo is tuple:", issubclass(codecs.CodecInfo, tuple))
for enc in [
    "utf-8",
    "UTF_8",
    "utf8",
    "utf-8-sig",
    "utf-16",
    "utf-16-le",
    "utf-16-be",
    "utf-32",
    "utf-32-le",
    "utf-32-be",
    "ascii",
    "latin-1",
    "iso-8859-1",
    "cp1252",
    "raw_unicode_escape",
    "unicode_escape",
    "utf-7",
    "rot_13",
    "hex",
    "base64",
    "zlib",
]:
    info = codecs.lookup(enc)
    print("lookup", enc, "->", info.name, len(info), info._is_text_encoding)

try:
    codecs.lookup("totally_unknown_encoding_3203")
except LookupError as e:
    print("unknown encoding:", str(e))

# 3. Round-trips for text encodings
sample = "Hello, world! café 日本語 😀 €"
for enc in ["utf-8", "utf-8-sig", "utf-16", "utf-16-le", "utf-16-be", "utf-32", "utf-32-le", "utf-32-be", "unicode_escape", "raw_unicode_escape", "utf-7"]:
    encoded = codecs.encode(sample, enc)
    decoded = codecs.decode(encoded, enc)
    print("roundtrip", enc, isinstance(encoded, bytes), decoded == sample)

# UTF-8-SIG BOM check
sig_bytes = codecs.encode("hi", "utf-8-sig")
print("utf-8-sig prefix:", sig_bytes.startswith(codecs.BOM_UTF8), codecs.decode(sig_bytes, "utf-8-sig"))
print("utf-8-sig no-bom:", codecs.decode(b"hi", "utf-8-sig"))

# ASCII / Latin-1 / CP1252
print("ascii:", codecs.encode("abc", "ascii"), codecs.decode(b"abc", "ascii"))
print("latin-1:", codecs.encode("caf\xe9", "latin-1"), codecs.decode(b"caf\xe9", "latin-1"))
print("cp1252:", codecs.encode("\u20ac\u201c\u201d", "cp1252"), codecs.decode(b"\x80\x93\x94", "cp1252"))

# 4. Non-text transforms via codecs.encode / codecs.decode (NOT str.encode / bytes.decode)
print("rot_13 enc:", codecs.encode("Hello, World! 123", "rot_13"))
print("rot_13 dec:", codecs.decode("Uryyb, Jbeyq! 123", "rot_13"))
print("hex enc:", codecs.encode(b"\x00\xab\xcd\xff", "hex"))
print("hex dec:", codecs.decode(b"00abcdff", "hex"))
print("hex dec str:", codecs.decode("00abcdff", "hex"))
print("base64 enc:", codecs.encode(b"hello world", "base64"))
print("base64 dec:", codecs.decode(b"aGVsbG8gd29ybGQ=\n", "base64"))
z = codecs.encode(b"hello " * 20, "zlib")
print("zlib rt:", codecs.decode(z, "zlib") == b"hello " * 20)

for expr_name, fn in [
    ("str.encode rot_13", lambda: "hello".encode("rot_13")),
    ("bytes.decode hex", lambda: b"6865".decode("hex")),
    ("bytes.decode base64", lambda: b"aGVsbG8=\n".decode("base64")),
    ("bytes.decode zlib", lambda: z.decode("zlib")),
]:
    try:
        fn()
        print("UNEXPECTED SUCCESS:", expr_name)
    except LookupError:
        print("non-text blocked:", expr_name, "LookupError")

# 5. Built-in error handlers
bad_str = "a\xe9\u2603b"
print("err ignore:", codecs.encode(bad_str, "ascii", "ignore"))
print("err replace:", codecs.encode(bad_str, "ascii", "replace"))
print("err xmlcharrefreplace:", codecs.encode(bad_str, "ascii", "xmlcharrefreplace"))
print("err backslashreplace:", codecs.encode(bad_str, "ascii", "backslashreplace"))
print("err namereplace:", codecs.encode(bad_str, "ascii", "namereplace"))

bad_bytes = b"a\xff\x80b"
print("dec ignore:", codecs.decode(bad_bytes, "utf-8", "ignore"))
print("dec replace:", codecs.decode(bad_bytes, "utf-8", "replace"))
print("dec backslashreplace:", codecs.decode(bad_bytes, "utf-8", "backslashreplace"))
se_str = codecs.decode(bad_bytes, "utf-8", "surrogateescape")
print("dec surrogateescape:", [hex(ord(c)) for c in se_str])
print("enc surrogateescape:", codecs.encode(se_str, "utf-8", "surrogateescape"))

# Direct error handler calls
e_enc = UnicodeEncodeError("ascii", "a\xe9b", 1, 2, "ordinal not in range(128)")
e_dec = UnicodeDecodeError("utf-8", b"a\xffb", 1, 2, "invalid start byte")
e_trn = UnicodeTranslateError("a\xe9b", 1, 2, "cannot translate")
print("strict_errors:", end=" ")
try:
    codecs.strict_errors(e_enc)
except UnicodeEncodeError:
    print("raised")
print("ignore_errors:", codecs.ignore_errors(e_enc), codecs.ignore_errors(e_dec), codecs.ignore_errors(e_trn))
print("replace_errors:", codecs.replace_errors(e_enc), codecs.replace_errors(e_dec), codecs.replace_errors(e_trn))
print("xmlcharrefreplace_errors:", codecs.xmlcharrefreplace_errors(e_enc))
print("backslashreplace_errors:", codecs.backslashreplace_errors(e_enc), codecs.backslashreplace_errors(e_dec))
print("namereplace_errors:", codecs.namereplace_errors(e_enc))

# 6. Custom error handlers via register_error / lookup_error
def brackets_handler(exc):
    if isinstance(exc, UnicodeEncodeError):
        return ("".join(f"[U+{ord(c):04X}]" for c in exc.object[exc.start:exc.end]), exc.end)
    if isinstance(exc, UnicodeDecodeError):
        return ("".join(f"[0x{b:02X}]" for b in exc.object[exc.start:exc.end]), exc.end)
    raise TypeError("unsupported")

codecs.register_error("brackets_3203", brackets_handler)
print("lookup_error custom:", codecs.lookup_error("brackets_3203") is brackets_handler)
print("custom enc:", codecs.encode("hi\u2603!", "ascii", "brackets_3203"))
print("custom dec:", codecs.decode(b"hi\xff!", "utf-8", "brackets_3203"))

def raw_bytes_handler(exc):
    return (b"<RAW>", exc.end)

codecs.register_error("raw_bytes_3203", raw_bytes_handler)
print("custom bytes enc:", codecs.encode("a\xe9b", "ascii", "raw_bytes_3203"))

try:
    codecs.lookup_error("no_such_error_handler_3203")
except LookupError as e:
    print("unknown error handler:", str(e))

# 7. Incremental encoders & decoders (including multi-byte UTF-8 / UTF-16 / UTF-32 split across chunks!)
text_mb = "Aé中😀Z"
utf8_raw = text_mb.encode("utf-8")
inc_dec = codecs.getincrementaldecoder("utf-8")()
chunks_out = []
for b in utf8_raw:
    chunks_out.append(inc_dec.decode(bytes([b]), final=False))
chunks_out.append(inc_dec.decode(b"", final=True))
print("utf-8 byte-by-byte:", "".join(chunks_out) == text_mb, chunks_out)

# State save/restore mid-sequence
inc_dec2 = codecs.getincrementaldecoder("utf-8")()
full_cjk = "日本語".encode("utf-8")
p1 = inc_dec2.decode(full_cjk[:4], final=False)
st = inc_dec2.getstate()
inc_dec3 = codecs.getincrementaldecoder("utf-8")()
inc_dec3.setstate(st)
p2 = inc_dec3.decode(full_cjk[4:], final=True)
print("utf-8 state restore:", p1 + p2 == "日本語", p1, p2)

# UTF-8-SIG split BOM across chunks
sig_raw = codecs.BOM_UTF8 + "hello".encode("utf-8")
sig_dec = codecs.getincrementaldecoder("utf-8-sig")()
sig_parts = [sig_dec.decode(bytes([b]), False) for b in sig_raw]
sig_parts.append(sig_dec.decode(b"", True))
print("utf-8-sig byte-by-byte:", "".join(sig_parts))

sig_enc = codecs.getincrementalencoder("utf-8-sig")()
se1 = sig_enc.encode("he", False)
se2 = sig_enc.encode("llo", True)
print("utf-8-sig inc enc:", se1, se2)

# UTF-16 split BOM + surrogate pair across single bytes
u16_raw = codecs.encode("A😀B", "utf-16")
u16_dec = codecs.getincrementaldecoder("utf-16")()
u16_out = "".join(u16_dec.decode(bytes([b]), False) for b in u16_raw) + u16_dec.decode(b"", True)
print("utf-16 byte-by-byte:", u16_out == "A😀B")

# UTF-32 split across single bytes
u32_raw = codecs.encode("A😀B", "utf-32")
u32_dec = codecs.getincrementaldecoder("utf-32")()
u32_out = "".join(u32_dec.decode(bytes([b]), False) for b in u32_raw) + u32_dec.decode(b"", True)
print("utf-32 byte-by-byte:", u32_out == "A😀B")

# Incomplete trailing sequence at final=True raises UnicodeDecodeError
inc_fail = codecs.getincrementaldecoder("utf-8")()
print("partial buffered:", repr(inc_fail.decode(b"\xe4\xb8", final=False)))
try:
    inc_fail.decode(b"", final=True)
except UnicodeDecodeError as e:
    print("final incomplete err:", e.encoding, e.reason)

# 8. iterencode & iterdecode
enc_chunks = list(codecs.iterencode(["Hello ", "世界", "!"], "utf-8"))
print("iterencode:", b"".join(enc_chunks) == "Hello 世界!".encode("utf-8"))
dec_chunks = list(codecs.iterdecode([utf8_raw[i : i + 1] for i in range(len(utf8_raw))], "utf-8"))
print("iterdecode:", "".join(dec_chunks) == text_mb)

# 9. StreamReader, StreamWriter, StreamReaderWriter, StreamRecoder, EncodedFile
buf = io.BytesIO()
writer = codecs.getwriter("utf-8")(buf)
writer.write("line1 café\n")
writer.writelines(["line2 日本語\n", "line3\n"])
buf.seek(0)
reader = codecs.getreader("utf-8")(buf)
print("stream readline1:", repr(reader.readline()))
print("stream rest lines:", list(reader))

# EncodedFile / StreamRecoder (data_encoding=utf-8, file_encoding=utf-16)
raw_buf = io.BytesIO()
ef = codecs.EncodedFile(raw_buf, "utf-8", "utf-16")
ef.write("café 日本語".encode("utf-8"))
raw_buf.seek(0)
print("encodedfile raw is utf-16:", raw_buf.getvalue().decode("utf-16"))
ef.seek(0)
print("encodedfile read back:", ef.read().decode("utf-8"))

# 10. Custom codec via register / unregister
class ReverseCodec(codecs.Codec):
    def encode(self, input, errors="strict"):
        return (input[::-1].encode("utf-8", errors), len(input))

    def decode(self, input, errors="strict"):
        return (bytes(input).decode("utf-8", errors)[::-1], len(input))

class ReverseIncEncoder(codecs.IncrementalEncoder):
    def encode(self, input, final=False):
        return input[::-1].encode("utf-8", self.errors)

class ReverseIncDecoder(codecs.IncrementalDecoder):
    def decode(self, input, final=False):
        return bytes(input).decode("utf-8", self.errors)[::-1]

class ReverseStreamWriter(ReverseCodec, codecs.StreamWriter):
    pass

class ReverseStreamReader(ReverseCodec, codecs.StreamReader):
    pass

def search_reverse(name):
    if name == "reverse_3203":
        return codecs.CodecInfo(
            name="reverse-3203",
            encode=ReverseCodec().encode,
            decode=ReverseCodec().decode,
            incrementalencoder=ReverseIncEncoder,
            incrementaldecoder=ReverseIncDecoder,
            streamreader=ReverseStreamReader,
            streamwriter=ReverseStreamWriter,
        )
    return None

codecs.register(search_reverse)
print("custom lookup:", codecs.lookup("REVERSE-3203").name)
print("custom encode:", codecs.encode("abcdef", "reverse-3203"))
print("custom decode:", codecs.decode(b"fedcba", "reverse-3203"))
codecs.unregister(search_reverse)
try:
    codecs.lookup("reverse-3203")
    print("UNEXPECTED: still registered")
except LookupError:
    print("unregistered ok")

# 11. codecs.open
with tempfile.TemporaryDirectory() as tmpdir:
    fpath = os.path.join(tmpdir, "sample_utf16.txt")
    with codecs.open(fpath, "w", encoding="utf-16") as f:
        f.write("Hello café\n日本語 line 2\n")
    with codecs.open(fpath, "r", encoding="utf-16") as f:
        print("codecs.open encoding:", f.encoding)
        print("codecs.open lines:", f.readlines())
    with codecs.open(fpath, "r", encoding="utf-16") as f:
        print("codecs.open iter:", [line.strip() for line in f])
    # encoding=None opens raw file
    fpath2 = os.path.join(tmpdir, "plain.txt")
    with codecs.open(fpath2, "w") as f:
        f.write("plain text")
    with codecs.open(fpath2, "r") as f:
        print("codecs.open none:", repr(f.read()))

# 12. Helper functions: make_identity_dict, make_encoding_map, charmap_build
id_map = codecs.make_identity_dict(range(4))
print("identity_dict:", id_map)
enc_map = codecs.make_encoding_map({0: 65, 1: 66, 2: 65})
print("encoding_map:", sorted(enc_map.items()))
cm = codecs.charmap_build("ABC\ufffe")
print("charmap_encode:", codecs.charmap_encode("CBA", "strict", cm))
print("charmap_decode:", codecs.charmap_decode(b"\x02\x01\x00", "strict", "ABC\ufffe"))

# 13. BufferedIncrementalEncoder, StreamReaderWriter, surrogatepass, escape_encode/decode
class PairBufEncoder(codecs.BufferedIncrementalEncoder):
    def _buffer_encode(self, input, errors, final):
        if not final and len(input) % 2 == 1:
            n = len(input) - 1
            return (input[:n].encode("ascii"), n)
        return (input.encode("ascii"), len(input))

pbe = PairBufEncoder()
print("buf_inc_enc:", pbe.encode("a", False), pbe.encode("bc", False), pbe.encode("d", True))
pbe.reset()
print("buf_inc_enc state:", pbe.getstate())

srw_buf = io.BytesIO()
srw = codecs.StreamReaderWriter(
    srw_buf, codecs.getreader("utf-8"), codecs.getwriter("utf-8"), "strict"
)
srw.write("hello café\n")
srw.seek(0)
print("srw read:", repr(srw.read()))

# surrogatepass roundtrip in UTF-8
sur = "a\ud800b"
sur_b = codecs.encode(sur, "utf-8", "surrogatepass")
print("surrogatepass utf-8:", sur_b, [hex(ord(c)) for c in codecs.decode(sur_b, "utf-8", "surrogatepass")])

# escape_encode / escape_decode
esc_b, esc_n = codecs.escape_encode(b"a\nb\x00\\'")
print("escape_encode:", esc_b, esc_n)
print("escape_decode:", codecs.escape_decode(esc_b))

# Custom error handler returning negative index
def neg_idx_handler(exc):
    return ("!", -1)

codecs.register_error("neg_idx_3203", neg_idx_handler)
print("neg_idx enc:", codecs.encode("a\xe9\xe9b", "ascii", "neg_idx_3203"))

# Zlib incremental encoder/decoder
z_inc_e = codecs.getincrementalencoder("zlib")()
z_inc_d = codecs.getincrementaldecoder("zlib")()
z_part = z_inc_e.encode(b"hello ", False) + z_inc_e.encode(b"world" * 10, True)
z_out = z_inc_d.decode(z_part[:5], False) + z_inc_d.decode(z_part[5:], True)
print("zlib incremental:", z_out == b"hello " + b"world" * 10)
