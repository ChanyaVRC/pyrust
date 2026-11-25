# Comprehensive CPython 3.12 parity tests for `encodings`, `encodings.aliases`,
# standard codec submodules, `encodings.punycode`, and `encodings.idna` (issue #3250).

import codecs
import encodings
import encodings.aliases
import encodings.ascii
import encodings.base64_codec
import encodings.cp1252
import encodings.hex_codec
import encodings.idna
import encodings.latin_1
import encodings.punycode
import encodings.raw_unicode_escape
import encodings.rot_13
import encodings.unicode_escape
import encodings.utf_7
import encodings.utf_8
import encodings.utf_8_sig
import encodings.utf_16
import encodings.utf_16_be
import encodings.utf_16_le
import encodings.utf_32
import encodings.utf_32_be
import encodings.utf_32_le
import encodings.zlib_codec
import io

# 1. CodecRegistryError hierarchy
print(
    "CodecRegistryError bases:",
    issubclass(encodings.CodecRegistryError, LookupError),
    issubclass(encodings.CodecRegistryError, SystemError),
)

# 2. normalize_encoding
for raw in [
    "utf-8",
    "UTF-8",
    "  - ; utf-8 - ",
    "latin.1",
    "iso-8859-1",
    "windows-1252",
    b"utf-8",
    b"ASCII",
    "---",
    "",
]:
    print("norm", repr(raw), "->", repr(encodings.normalize_encoding(raw)))

# 3. encodings.aliases.aliases spot checks
al = encodings.aliases.aliases
for key in [
    "utf8",
    "u8",
    "latin1",
    "iso8859_1",
    "us_ascii",
    "windows_1252",
    "utf16",
    "utf32",
    "hex",
    "base64",
    "rot13",
    "zip",
    "zlib",
]:
    print("alias", key, "->", al.get(key))
print("aliases count >= 300:", len(al) >= 300)

# 4. encodings.search_function
for enc in [
    "utf-8",
    "utf8",
    "latin1",
    "iso8859-1",
    "us-ascii",
    "windows-1252",
    "utf-16",
    "utf-32",
    "idna",
    "punycode",
    "hex",
    "base64",
    "rot13",
    "UTF-8",
    "totally_unknown_codec_3250",
]:
    info = encodings.search_function(enc)
    if info is None:
        print("search", enc, "-> None")
    else:
        print("search", enc, "->", info.name, isinstance(info, codecs.CodecInfo))

# 5. Standard codec submodules surface & round-trips
for mod_name, mod in [
    ("utf_8", encodings.utf_8),
    ("utf_8_sig", encodings.utf_8_sig),
    ("ascii", encodings.ascii),
    ("latin_1", encodings.latin_1),
    ("utf_16", encodings.utf_16),
    ("utf_16_le", encodings.utf_16_le),
    ("utf_16_be", encodings.utf_16_be),
    ("utf_32", encodings.utf_32),
    ("utf_32_le", encodings.utf_32_le),
    ("utf_32_be", encodings.utf_32_be),
    ("cp1252", encodings.cp1252),
    ("raw_unicode_escape", encodings.raw_unicode_escape),
    ("unicode_escape", encodings.unicode_escape),
    ("utf_7", encodings.utf_7),
    ("hex_codec", encodings.hex_codec),
    ("base64_codec", encodings.base64_codec),
    ("rot_13", encodings.rot_13),
    ("zlib_codec", encodings.zlib_codec),
]:
    reg = mod.getregentry()
    has_all = all(
        hasattr(mod, attr)
        for attr in (
            "getregentry",
            "IncrementalEncoder",
            "IncrementalDecoder",
            "StreamReader",
            "StreamWriter",
        )
    )
    print("submod", mod_name, reg.name, isinstance(reg, codecs.CodecInfo), has_all)

print("utf_8 encode/decode:", encodings.utf_8.encode("café"), encodings.utf_8.decode(b"caf\xc3\xa9"))
print("ascii encode/decode:", encodings.ascii.Codec().encode("abc"), encodings.ascii.Codec().decode(b"abc"))
print("latin_1 encode/decode:", encodings.latin_1.Codec().encode("caf\xe9"), encodings.latin_1.Codec().decode(b"caf\xe9"))
print("cp1252 encode/decode:", encodings.cp1252.Codec().encode("\u20ac"), encodings.cp1252.Codec().decode(b"\x80"))

# 6. encodings.punycode (RFC 3492)
for text in [
    "münchen",
    "bücher",
    "日本語",
    "пример",
    "ascii",
    "a-b-c",
    "München-3ya",
    "mañana",
    "",
]:
    enc = encodings.punycode.punycode_encode(text)
    dec = encodings.punycode.punycode_decode(enc, "strict")
    print("punycode", repr(text), "->", enc, "rt:", dec == text)

pc_info = encodings.punycode.getregentry()
print("punycode reg:", pc_info.name, isinstance(pc_info, codecs.CodecInfo))
print("punycode codecs.encode:", codecs.encode("bücher", "punycode"))
print("punycode codecs.decode:", codecs.decode(b"bcher-kva", "punycode"))

# Punycode helper functions
print("punycode T:", encodings.punycode.T(0, 72), encodings.punycode.T(3, 72))
print("punycode adapt:", encodings.punycode.adapt(0, True, 1), encodings.punycode.adapt(100, False, 5))
print("punycode segregate:", encodings.punycode.segregate("münchen"))

# Punycode incremental & stream
pc_ie = encodings.punycode.IncrementalEncoder()
pc_id = encodings.punycode.IncrementalDecoder()
print("punycode inc:", pc_ie.encode("bücher"), pc_id.decode(b"bcher-kva"))

# 7. encodings.idna (RFC 3490 / RFC 3491)
print("idna prefixes:", encodings.idna.ace_prefix, encodings.idna.sace_prefix)
print("idna nameprep:", repr(encodings.idna.nameprep("München")), repr(encodings.idna.nameprep("ß")))

for label in ["example", "münchen", "bücher", "日本語", "пример", "STRAẞE"]:
    a = encodings.idna.ToASCII(label)
    u = encodings.idna.ToUnicode(a)
    print("idna label", repr(label), "->", a, "->", repr(u))

for domain in [
    "example.com",
    "münchen.de",
    "bücher.example.com.",
    "日本語.jp",
    "example\u3002com",
    "",
]:
    e, n = encodings.idna.Codec().encode(domain)
    d, m = encodings.idna.Codec().decode(e)
    print("idna domain", repr(domain), "->", e, n, "->", repr(d), m)

# IDNA via codecs.encode / codecs.decode
print("idna codecs:", codecs.encode("münchen.de", "idna"), codecs.decode(b"xn--mnchen-3ya.de", "idna"))

# IDNA incremental encoder & decoder
idna_ie = encodings.idna.IncrementalEncoder()
print(
    "idna inc enc:",
    idna_ie.encode("münchen.", False),
    idna_ie.encode("de", True),
)
idna_id = encodings.idna.IncrementalDecoder()
print(
    "idna inc dec:",
    idna_id.decode(b"xn--mnchen-3ya.", False),
    idna_id.decode(b"de", True),
)

# IDNA stream writer & reader
buf = io.BytesIO()
sw = encodings.idna.StreamWriter(buf)
sw.write("münchen.de")
buf.seek(0)
sr = encodings.idna.StreamReader(buf)
print("idna stream:", sr.read())

# IDNA error checks
for bad_label in ["", "a" * 64, "a\u1680b", "a\ue000b"]:
    try:
        encodings.idna.ToASCII(bad_label)
        print("UNEXPECTED IDNA SUCCESS:", repr(bad_label))
    except UnicodeError:
        print("idna error ok:", len(bad_label))

# BiDi violation checks
try:
    encodings.idna.nameprep("\u05d0a")
    print("UNEXPECTED BIDI2")
except UnicodeError as e:
    print("bidi2:", str(e))

try:
    encodings.idna.nameprep("\u05d01")
    print("UNEXPECTED BIDI3")
except UnicodeError as e:
    print("bidi3:", str(e))
