"""Parity fixture for the 100% native `base64` module (issue #3487).

Everything printed here must be byte-for-byte identical to CPython 3.12.3.
"""

import array
import base64
import binascii
import io
import sys


def run(fn, *args, **kwargs):
    try:
        return ("OK", fn(*args, **kwargs))
    except Exception as e:
        return (type(e).__name__, str(e))


def kind(fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
        return "OK"
    except Exception as e:
        return type(e).__name__


all_bytes = bytes(range(256))
samples = [bytes(range(n)) for n in range(33)] + [all_bytes, b"\x00" * 9, b" " * 9]

# ---------------------------------------------------------------------------
# 1. Module metadata
# ---------------------------------------------------------------------------
print("__all__:", base64.__all__)
print("limits:", base64.MAXLINESIZE, base64.MAXBINSIZE)
print("bytes_types:", base64.bytes_types)
print("doc:", base64.__doc__)
print("has re/struct/binascii:", base64.re.__name__, base64.struct.__name__, base64.binascii is binascii)
for name in base64.__all__:
    assert callable(getattr(base64, name)), name
print("b64encode doc:", base64.b64encode.__doc__.splitlines()[0])
print("b32encode doc:", repr(base64.b32encode.__doc__) if base64.b32encode.__doc__ else None)
print("b16decode doc first:", base64.b16decode.__doc__.splitlines()[0])
print("alphabets:", base64._b32alphabet, base64._b32hexalphabet)
print("urlsafe tables:", base64._urlsafe_encode_translation[43], base64._urlsafe_decode_translation[45])

# ---------------------------------------------------------------------------
# 2. Every encoding over lengths 0..32 and all 256 byte values
# ---------------------------------------------------------------------------
encoders = [
    "b64encode",
    "standard_b64encode",
    "urlsafe_b64encode",
    "b32encode",
    "b32hexencode",
    "b16encode",
    "a85encode",
    "b85encode",
    "encodebytes",
]
decoders = {
    "b64encode": "b64decode",
    "standard_b64encode": "standard_b64decode",
    "urlsafe_b64encode": "urlsafe_b64decode",
    "b32encode": "b32decode",
    "b32hexencode": "b32hexdecode",
    "b16encode": "b16decode",
    "a85encode": "a85decode",
    "b85encode": "b85decode",
    "encodebytes": "decodebytes",
}
for enc_name in encoders:
    enc = getattr(base64, enc_name)
    dec = getattr(base64, decoders[enc_name])
    for sample in samples:
        encoded = enc(sample)
        assert isinstance(encoded, bytes), (enc_name, type(encoded))
        assert dec(encoded) == sample, (enc_name, sample)
        if enc_name != "encodebytes":
            assert dec(encoded.decode("ascii")) == sample, (enc_name, sample)
    print(enc_name, [enc(s).hex() for s in samples[:12]])
    print(enc_name, "all256", enc(all_bytes).hex())

# ---------------------------------------------------------------------------
# 3. b64encode / b64decode: altchars, validate, padding
# ---------------------------------------------------------------------------
print(run(base64.b64encode, b"\xfb\xff\xfe", altchars=b"-_"))
print(run(base64.b64encode, b"\xfb\xff\xfe", b"*!"))
print(run(base64.b64encode, b"\xfb\xff\xfe", bytearray(b"*!")))
print(run(base64.b64encode, b"\xfb\xff\xfe", memoryview(b"@#")))
print(run(base64.b64encode, b"abc", None))
print(run(base64.b64encode, b"abc", b"-"))
print(run(base64.b64encode, b"abc", b"-_/"))
print(run(base64.b64encode, b"abc", "-_"))
print(run(base64.b64encode, b"abc", 12))
print(run(base64.b64encode, b"abc", [1, 2]))
print(run(base64.b64encode, "abc"))
print(run(base64.b64encode, 12))
print(run(base64.b64encode, None))
print(run(base64.b64decode, b"-_-_", altchars=b"-_"))
print(run(base64.b64decode, b"-_-_", b"+/"))
print(run(base64.b64decode, "*!*!", "*!"))
print(run(base64.b64decode, b"abcd", b"-"))
print(run(base64.b64decode, b"abcd", "-_-"))
print(run(base64.b64decode, b"abcd", 5))
print(run(base64.b64decode, 5))
print(run(base64.b64decode, "caf\xe9"))
print(run(base64.b64decode, "€"))
print(run(base64.b64decode, None))
print(run(base64.b64decode, [1, 2, 3]))
print(run(base64.b64decode, b"YWJj"))
print(run(base64.b64decode, b"YWJj\n"))
print(run(base64.b64decode, b"YW Jj"))
print(run(base64.b64decode, "YWJj"))
print(run(base64.b64decode, bytearray(b"YWJj")))
print(run(base64.b64decode, memoryview(b"YWJj")))
print(run(base64.b64decode, array.array("B", b"YWJj")))
for text in [
    b"YQ==",
    b"YQ=",
    b"YQ",
    b"Y",
    b"YWI=",
    b"YWI",
    b"=YWJj",
    b"YQ==YQ==",
    b"YQ=a",
    b"YQ==\n",
    b"Y!Q==",
    b"YW$Jj",
    b"YWJj!",
    b"====",
    b"=",
    b"",
    b"\xff\xfeYWJj",
    b"YW\xffJj",
    b"YWJjYQ=",
    b"YWJjYQ==YWJj",
    b"YWJjY",
    b"YWJjYQ===",
]:
    print(text, run(base64.b64decode, text), run(base64.b64decode, text, validate=True), run(base64.standard_b64decode, text))
# Trailing '=' after a complete 4-char quad: CPython 3.12.4 (GH-118761) tightened
# strict_mode=True here compared to 3.12.3; test non-strict parity across all 3.12.x.
for text in (b"YWJj====", b"YWJj=", b"YWJj==", b"YWJj=a"):
    print(text, run(base64.b64decode, text), run(base64.standard_b64decode, text))
print(run(base64.b64decode, b"YQ==", None, True))
print(run(base64.b64decode, b"YQ==", validate=1))
print(run(base64.b64decode, b"YQ", validate=[]))
print(run(base64.b64decode, b"a-b_c", b"-_", True))

# standard / urlsafe
print(run(base64.standard_b64decode, b"-_-_"))
print(run(base64.urlsafe_b64decode, b"-_-_"))
print(run(base64.urlsafe_b64decode, "-_-_"))
print(run(base64.urlsafe_b64decode, b"+/+/"))
print(run(base64.urlsafe_b64decode, b"YQ"))
print(run(base64.urlsafe_b64decode, 1))
print(run(base64.urlsafe_b64decode, "\xe9"))
print(run(base64.urlsafe_b64encode, all_bytes))
print(run(base64.urlsafe_b64encode, b"\xfb\xff\xfe\xff"))
print(run(base64.urlsafe_b64encode, "x"))
print(run(base64.standard_b64encode, b"\xfb\xff\xfe\xff"))
print(run(base64.standard_b64encode, "x"))

# bytes-like inputs on every encoder
for enc_name in encoders:
    enc = getattr(base64, enc_name)
    ref = enc(b"hello world!")
    print(enc_name, "bytearray", enc(bytearray(b"hello world!")) == ref)
    print(enc_name, "memoryview", enc(memoryview(b"hello world!")) == ref)
    print(enc_name, "array", run(enc, array.array("B", b"hello world!")))
    print(enc_name, "str", run(enc, "hello"))
    print(enc_name, "int", run(enc, 5))
    print(enc_name, "None", run(enc, None))
    print(enc_name, "list", run(enc, [1, 2]))
    print(enc_name, "missing", run(enc))
    print(enc_name, "extra", run(enc, b"", 1, 2, 3, 4, 5))

class MyBytes(bytes):
    pass


class MyStr(str):
    pass


class MyByteArray(bytearray):
    pass


for enc_name in encoders:
    enc = getattr(base64, enc_name)
    print(enc_name, "subclasses", enc(MyBytes(b"abcdef")) == enc(b"abcdef"), enc(MyByteArray(b"abcdef")) == enc(b"abcdef"))
for dec_name in ("b64decode", "standard_b64decode", "urlsafe_b64decode", "b32decode", "b32hexdecode", "b16decode", "a85decode", "b85decode"):
    dec = getattr(base64, dec_name)
    enc = getattr(base64, {v: k for k, v in decoders.items()}[dec_name])
    text = enc(b"subclass")
    print(dec_name, "subclasses", dec(MyBytes(text)), dec(MyByteArray(text)), dec(MyStr(text.decode())))


class Exporter:
    def __buffer__(self, flags):
        return memoryview(b"exported")

    def __release_buffer__(self, view):
        view.release()


print(run(base64.b64encode, Exporter()))
print(run(base64.b32encode, Exporter()))
print(run(base64.b16encode, Exporter()))
print(run(base64.a85encode, Exporter()))
print(run(base64.b85encode, Exporter()))
print(run(base64.b64decode, Exporter()))
print(run(base64.b32decode, Exporter()))

# ---------------------------------------------------------------------------
# 4. Base32 / Base32hex
# ---------------------------------------------------------------------------
print(run(base64.b32encode, b""))
print(run(base64.b32encode, b"a"))
print(run(base64.b32encode, b"ab"))
print(run(base64.b32encode, b"abc"))
print(run(base64.b32encode, b"abcd"))
print(run(base64.b32encode, b"abcde"))
print(run(base64.b32hexencode, b"abcd"))
for text in [
    b"ME======",
    b"MFRA====",
    b"MFRGG===",
    b"MFRGGZA=",
    b"MFRGGZDF",
    b"me======",
    b"Me======",
    b"ME=====",
    b"ME======X",
    b"ME",
    b"ME=======",
    b"M=======",
    b"========",
    b"MFRGGZDFMFRGGZDF",
    b"MFRGGZDFME======",
    b"MFRGGZDF========",
    b"MFRGG===MFRGG===",
    b"MFRG=ZA=",
    b"MFRGGZA==",
    b"MFRG1ZA=",
    b"MFRG0ZA=",
    b"MFR!GZDF",
    b"ME\xff=====",
    b"",
    b"A",
    b"AAAAAAAA",
    b"AAAAAAA=",
    b"AAAAAA==",
    b"AAAAA===",
    b"AAAA====",
    b"AAA=====",
    b"AA======",
    b"A=======",
    "MFRGGZDF",
    "MFRG€ZDF",
]:
    print(text, run(base64.b32decode, text), run(base64.b32decode, text, True), run(base64.b32hexdecode, text), run(base64.b32hexdecode, text, casefold=True))
print(run(base64.b32decode, b"MFRG1ZA=", map01=b"L"))
print(run(base64.b32decode, b"MFRG1ZA=", map01=b"l"))
print(run(base64.b32decode, b"MFRG1ZA=", map01=b"l", casefold=True))
print(run(base64.b32decode, b"MFRG1ZA=", map01="I"))
print(run(base64.b32decode, b"MFRG1ZA=", map01=bytearray(b"I")))
print(run(base64.b32decode, b"MFRG0ZA=", map01=b"L"))
print(run(base64.b32decode, b"MFRG0ZA=", None, False, None))
print(run(base64.b32decode, b"MFRG1ZA=", False, b"LL"))
print(run(base64.b32decode, b"MFRG1ZA=", False, b""))
print(run(base64.b32decode, b"MFRG1ZA=", False, "€"))
print(run(base64.b32decode, b"MFRG1ZA=", False, 1))
print(run(base64.b32decode, b"MFRG1ZA=", False, 0))
print(run(base64.b32decode, b"MFRG1ZA=", True, b"I"))
print(run(base64.b32decode, b"MFRG1ZA", False, b"LL"))
print(run(base64.b32decode, 5))
print(run(base64.b32decode, None))
print(run(base64.b32decode, b"ME======", casefold=0))
print(run(base64.b32decode, b"me======", casefold="yes"))
print(run(base64.b32decode, b"me======", casefold=[]))
print(run(base64.b32hexdecode, b"C4======", True, 3))
print(run(base64.b32decode, b"ME======", 1, 2, 3))
print(run(base64.b32hexdecode, b"C4======", True, b"L"))

# ---------------------------------------------------------------------------
# 5. Base16
# ---------------------------------------------------------------------------
print(run(base64.b16encode, b""))
print(run(base64.b16encode, all_bytes))
for text in [
    b"6162",
    b"616",
    b"61G2",
    b"6a62",
    b"6A62",
    b"",
    b" 61",
    b"61\n",
    "6162",
    "61€",
    b"0",
    b"ZZ",
    bytearray(b"6162"),
    memoryview(b"6162"),
    5,
    None,
]:
    label = text if isinstance(text, (bytes, str, bytearray, int, type(None))) else type(text).__name__
    print(label, run(base64.b16decode, text), run(base64.b16decode, text, True), run(base64.b16decode, text, casefold=True))

# ---------------------------------------------------------------------------
# 6. Ascii85
# ---------------------------------------------------------------------------
print(run(base64.a85encode, b""))
print(run(base64.a85encode, b"\x00\x00\x00\x00"))
print(run(base64.a85encode, b"\x00" * 8))
print(run(base64.a85encode, b"\x00"))
print(run(base64.a85encode, b"\x00", pad=True))
print(run(base64.a85encode, b"\x00" * 5))
print(run(base64.a85encode, b"\x00" * 5, pad=True))
print(run(base64.a85encode, b"    "))
print(run(base64.a85encode, b"    ", foldspaces=True))
print(run(base64.a85encode, b"        ", foldspaces=True))
print(run(base64.a85encode, b"   ", foldspaces=True))
print(run(base64.a85encode, b"   ", foldspaces=True, pad=True))
print(run(base64.a85encode, b"\xff\xff\xff\xff"))
print(run(base64.a85encode, b"\xff\xff\xff"))
print(run(base64.a85encode, b"\xff\xff\xff", pad=True))
print(run(base64.a85encode, b"Hello, World!"))
print(run(base64.a85encode, b"Hello, World!", adobe=True))
print(run(base64.a85encode, b"", adobe=True))
print(run(base64.a85encode, b"", adobe=True, wrapcol=2))
print(run(base64.a85encode, b"", wrapcol=5))
for wrapcol in [0, 1, 2, 3, 4, 5, 7, 10, 76, -1, -5, 1000, True, False, None, 2.5, "5", 3 + 0j]:
    for adobe in (False, True):
        print(wrapcol, adobe, run(base64.a85encode, b"Hello, World! 0123456789", wrapcol=wrapcol, adobe=adobe))
for n in range(0, 14):
    data = bytes(range(65, 65 + n))
    for kw in [{}, {"pad": True}, {"foldspaces": True}, {"adobe": True}, {"adobe": True, "wrapcol": 4, "pad": True}]:
        print(n, kw, base64.a85encode(data, **kw))
print(run(base64.a85encode, b"abc", True))
print(run(base64.a85encode, b"abc", foo=1))
print(run(base64.a85encode, b"abc", pad=1, adobe=1, foldspaces=1, wrapcol=1))
print(run(base64.a85encode, "abc"))
print(run(base64.a85encode))
print(run(base64.a85encode, b"abc", b="x"))

print(run(base64.a85decode, b""))
print(run(base64.a85decode, b"z"))
print(run(base64.a85decode, b"zz"))
print(run(base64.a85decode, b"y"))
print(run(base64.a85decode, b"y", foldspaces=True))
print(run(base64.a85decode, b"yy", foldspaces=True))
print(run(base64.a85decode, b"!!"))
print(run(base64.a85decode, b"!"))
print(run(base64.a85decode, b"!!!!!!"))
print(run(base64.a85decode, b"87cURD]i,\"Ebo80"))
print(run(base64.a85decode, "87cURD]i,\"Ebo80"))
print(run(base64.a85decode, b"87cUR D]i,\n\"Ebo80"))
print(run(base64.a85decode, b"87cUR D]i,\n\"Ebo80", ignorechars=b" "))
print(run(base64.a85decode, b"87cUR D]i,\n\"Ebo80", ignorechars=b" \n"))
print(run(base64.a85decode, b"87cUR D]i,\n\"Ebo80", ignorechars=bytearray(b" \n")))
print(run(base64.a85decode, b"87cUR D]i,\n\"Ebo80", ignorechars=[32, 10]))
print(run(base64.a85decode, b"87cUR D]i,\n\"Ebo80", ignorechars=(32, 10)))
print(run(base64.a85decode, b"87cUR D]i,\n\"Ebo80", ignorechars=5))
print(run(base64.a85decode, b"87cUR D]i,\n\"Ebo80", ignorechars=None))
print(run(base64.a85decode, b"87cUR\x0bD]i,\"Ebo80"))
print(run(base64.a85decode, b"87cUR\x0cD]i,\"Ebo80"))
print(run(base64.a85decode, b"87cUR\x0cD]i,\"Ebo80", ignorechars=b"\x0c"))
print(run(base64.a85decode, b"87z"))
print(run(base64.a85decode, b"8z"))
print(run(base64.a85decode, b"87cUz"))
print(run(base64.a85decode, b"87cUy", foldspaces=True))
print(run(base64.a85decode, b"87cURy", foldspaces=True))
print(run(base64.a85decode, b"87cURy", foldspaces=False))
print(run(base64.a85decode, b"s8W-!"))
print(run(base64.a85decode, b"s8W-\""))
print(run(base64.a85decode, b"s8W-"))
print(run(base64.a85decode, b"s8W"))
print(run(base64.a85decode, b"uuuuu"))
print(run(base64.a85decode, b"u"))
print(run(base64.a85decode, b"~"))
print(run(base64.a85decode, b"}"))
print(run(base64.a85decode, b"\xe9"))
print(run(base64.a85decode, b"\x00"))
print(run(base64.a85decode, b"abc\xffdef"))
print(run(base64.a85decode, "caf\xe9"))
print(run(base64.a85decode, 5))
print(run(base64.a85decode, None))
print(run(base64.a85decode, b"<~87cURD]i,\"Ebo80~>", adobe=True))
print(run(base64.a85decode, b"87cURD]i,\"Ebo80~>", adobe=True))
print(run(base64.a85decode, b"<~87cURD]i,\"Ebo80", adobe=True))
print(run(base64.a85decode, b"87cURD]i,\"Ebo80", adobe=True))
print(run(base64.a85decode, b"<~~>", adobe=True))
print(run(base64.a85decode, b"~>", adobe=True))
print(run(base64.a85decode, b"<~>", adobe=True))
print(run(base64.a85decode, b"<~", adobe=True))
print(run(base64.a85decode, b"", adobe=True))
print(run(base64.a85decode, b"<~\n87cURD]i,\n\"Ebo80\n~>", adobe=True))
print(run(base64.a85decode, b"<~87cURD]i,\"Ebo80~>", adobe=False))
print(run(base64.a85decode, b"<~87cURD]i,\"Ebo80~>trailing", adobe=True))
print(run(base64.a85decode, "<~87cURD]i,\"Ebo80~>", adobe=True))
print(run(base64.a85decode, b"87cURD]i,\"Ebo80", True))
print(run(base64.a85decode, b"87cURD]i,\"Ebo80", foldspaces=1, adobe=0, ignorechars=b" "))
print(run(base64.a85decode))
print(run(base64.a85decode, b"abc", bad=1))
for n in range(0, 14):
    data = bytes(range(65, 65 + n))
    for kw in [{}, {"pad": True}, {"foldspaces": True}, {"adobe": True}]:
        enc = base64.a85encode(data, **kw)
        dec_kw = {}
        if kw.get("adobe"):
            dec_kw["adobe"] = True
        if kw.get("foldspaces"):
            dec_kw["foldspaces"] = True
        got = base64.a85decode(enc, **dec_kw)
        if kw.get("pad"):
            assert got.startswith(data), (n, kw, got)
        else:
            assert got == data, (n, kw, got)
        print(n, kw, got)
for wrapcol in (1, 2, 5, 13, 76):
    enc = base64.a85encode(all_bytes, wrapcol=wrapcol, adobe=True, foldspaces=True)
    assert base64.a85decode(enc, adobe=True, foldspaces=True) == all_bytes
    assert max(len(line) for line in enc.split(b"\n")) <= max(wrapcol, 2)
    print(wrapcol, enc.count(b"\n"), enc[-5:])
big = bytes((i * 7919) % 256 for i in range(1000))
assert base64.a85decode(base64.a85encode(big, wrapcol=60, adobe=True), adobe=True) == big

# ---------------------------------------------------------------------------
# 7. Base85
# ---------------------------------------------------------------------------
print(run(base64.b85encode, b""))
print(run(base64.b85encode, b"\x00"))
print(run(base64.b85encode, b"\x00", pad=True))
print(run(base64.b85encode, b"\x00", True))
print(run(base64.b85encode, b"\x00" * 4))
print(run(base64.b85encode, b"\x00" * 5))
print(run(base64.b85encode, b"\x00" * 5, pad=True))
print(run(base64.b85encode, b"    "))
print(run(base64.b85encode, b"\xff\xff\xff\xff"))
print(run(base64.b85encode, b"\xff\xff\xff"))
print(run(base64.b85encode, b"\xff\xff\xff", pad=1))
print(run(base64.b85encode, b"hello"))
print(run(base64.b85encode, b"hello world"))
print(run(base64.b85encode, b"hello world", pad=True))
print(run(base64.b85encode, "hello"))
print(run(base64.b85encode, b"hello", 1, 2))
print(run(base64.b85encode, b"hello", nope=1))
for n in range(0, 14):
    data = bytes(range(65, 65 + n))
    print(n, base64.b85encode(data), base64.b85encode(data, pad=True))
print(run(base64.b85decode, b""))
print(run(base64.b85decode, b"Xk~0{Zv"))
print(run(base64.b85decode, "Xk~0{Zv"))
print(run(base64.b85decode, bytearray(b"Xk~0{Zv")))
print(run(base64.b85decode, memoryview(b"Xk~0{Zv")))
print(run(base64.b85decode, b"0"))
print(run(base64.b85decode, b"00"))
print(run(base64.b85decode, b"000"))
print(run(base64.b85decode, b"0000"))
print(run(base64.b85decode, b"00000"))
print(run(base64.b85decode, b"000000"))
print(run(base64.b85decode, b"|NsC0"))
print(run(base64.b85decode, b"|NsC1"))
print(run(base64.b85decode, b"|Ns9"))
print(run(base64.b85decode, b"|NsC"))
print(run(base64.b85decode, b"|Ns"))
print(run(base64.b85decode, b"~~~~~"))
print(run(base64.b85decode, b"00000~~~~~"))
print(run(base64.b85decode, b"0000000000~~~~~"))
print(run(base64.b85decode, b"Xk~0{Zv\"x"))
print(run(base64.b85decode, b"Xk~0{ Zv"))
print(run(base64.b85decode, b"Xk~0{\nZv"))
print(run(base64.b85decode, b"Xk~0\xe9"))
print(run(base64.b85decode, b"Xk~0[Zv"))
print(run(base64.b85decode, b"Xk~0,Zv"))
print(run(base64.b85decode, b"\x00\x00\x00\x00\x00"))
print(run(base64.b85decode, "caf\xe9"))
print(run(base64.b85decode, 5))
print(run(base64.b85decode, None))
print(run(base64.b85decode))
print(run(base64.b85decode, b"0", b"1"))
for alpha in ("0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~"):
    base64.b85decode(("0000" + alpha).encode())
bad = [chr(c) for c in range(256) if chr(c) not in "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~"]
print(len(bad), [run(base64.b85decode, ("00" + c + "00").encode("latin-1")) for c in bad[:6]])
print(all(kind(base64.b85decode, (c * 5).encode("latin-1")) == "ValueError" for c in bad))

# ---------------------------------------------------------------------------
# 8. Legacy interface: encodebytes / decodebytes / encode / decode
# ---------------------------------------------------------------------------
for n in (0, 1, 2, 3, 54, 55, 56, 57, 58, 59, 113, 114, 115, 116, 171, 172, 300):
    data = bytes((i * 31) % 256 for i in range(n))
    enc = base64.encodebytes(data)
    print(n, enc.count(b"\n"), [len(line) for line in enc.split(b"\n")][:4], enc[-6:])
    assert base64.decodebytes(enc) == data
    assert all(len(line) <= 76 for line in enc.split(b"\n"))
    out = io.BytesIO()
    base64.encode(io.BytesIO(data), out)
    assert out.getvalue() == enc
    back = io.BytesIO()
    base64.decode(io.BytesIO(enc), back)
    assert back.getvalue() == data
print(repr(base64.encodebytes(b"")))
print(repr(base64.encodebytes(b"\x00")))
print(repr(base64.encodebytes(bytearray(b"abc"))))
print(repr(base64.encodebytes(memoryview(b"abc"))))
print(repr(base64.encodebytes(memoryview(b"abcdef")[1:4])))
print(repr(base64.encodebytes(memoryview(bytearray(b"abc")))))
print(run(base64.encodebytes, "abc"))
print(run(base64.encodebytes, 5))
print(run(base64.encodebytes, None))
print(run(base64.encodebytes, [1, 2]))
print(run(base64.encodebytes, array.array("B", b"abc")))
print(run(base64.encodebytes, array.array("b", b"abc")))
print(run(base64.encodebytes, array.array("i", [1, 2, 3])))
print(run(base64.encodebytes, array.array("H", [1, 2, 3])))
print(run(base64.encodebytes, memoryview(b"abcdef").cast("H")))
print(run(base64.encodebytes, memoryview(b"abcdef").cast("B", shape=[2, 3])))
print(run(base64.encodebytes, memoryview(b"abcdef").cast("c")))
print(run(base64.encodebytes))
print(run(base64.decodebytes, b"YWJj\n"))
print(run(base64.decodebytes, b"YWJj"))
print(run(base64.decodebytes, bytearray(b"YWJj\n")))
print(run(base64.decodebytes, memoryview(b"YWJj\n")))
print(run(base64.decodebytes, "YWJj\n"))
print(run(base64.decodebytes, 5))
print(run(base64.decodebytes, None))
print(run(base64.decodebytes, b"YQ"))
print(run(base64.decodebytes, b"Y"))
print(run(base64.decodebytes, b"YQ=="))
print(run(base64.decodebytes, b"YQ==\nYQ==\n"))
print(run(base64.decodebytes, b"!@#$"))
print(run(base64.decodebytes, array.array("i", [1, 2, 3])))
print(run(base64._input_type_check, b"abc"))
print(run(base64._input_type_check, "abc"))
print(run(base64._input_type_check, array.array("d", [1.0])))
print(run(base64._bytes_from_decode_data, "abc"))
print(run(base64._bytes_from_decode_data, b"abc"))
print(run(base64._bytes_from_decode_data, bytearray(b"abc")))
print(run(base64._bytes_from_decode_data, 5))
print(run(base64._bytes_from_decode_data, "\xe9"))

try:
    base64.encodebytes("abc")
except TypeError as e:
    print("cause:", type(e.__cause__).__name__, e.__suppress_context__)
try:
    base64.b64decode(5)
except TypeError as e:
    print("cause:", e.__cause__, e.__suppress_context__)


class Reader:
    """Hands out at most `step` bytes per read() call."""

    def __init__(self, data, step):
        self.buf = io.BytesIO(data)
        self.step = step
        self.calls = []

    def read(self, n=-1):
        self.calls.append(n)
        return self.buf.read(min(n, self.step))

    def readline(self):
        return self.buf.readline()


for step in (1, 5, 56, 57, 100):
    data = bytes(range(200))
    reader = Reader(data, step)
    out = io.BytesIO()
    base64.encode(reader, out)
    assert out.getvalue() == base64.encodebytes(data), step
    print("step", step, len(reader.calls), reader.calls[:6])


class Sink:
    def __init__(self):
        self.writes = []

    def write(self, data):
        self.writes.append(data)


sink = Sink()
base64.encode(io.BytesIO(b"x" * 120), sink)
print([len(w) for w in sink.writes])
sink = Sink()
base64.decode(io.BytesIO(b"YWJj\nZGVm\n"), sink)
print(sink.writes)
print(run(base64.decode, io.BytesIO(b"YQ\n"), io.BytesIO()))
print(run(base64.decode, io.BytesIO(b"YWJj\nY\n"), io.BytesIO()))
print(run(base64.decode, io.BytesIO(b""), io.BytesIO()))
print(run(base64.encode, io.BytesIO(b""), io.BytesIO()))
print(run(base64.encode, io.BytesIO(b"abc"), None))
print(run(base64.encode, None, io.BytesIO()))
print(run(base64.encode, io.StringIO("abc"), io.BytesIO()))
print(run(base64.decode, io.StringIO("YWJj\n"), io.BytesIO()))
print(run(base64.encode, io.BytesIO(b"abc")))
print(run(base64.decode, 1, 2, 3))
print(run(base64.encode, input=io.BytesIO(b"abc"), output=io.BytesIO()))

# ---------------------------------------------------------------------------
# 9. Calling conventions and patching
# ---------------------------------------------------------------------------
print(run(base64.b64encode, s=b"abc"))
print(run(base64.b64encode, b"abc", altchars=None))
print(run(base64.b64encode, b"abc", s=b"abc"))
print(run(base64.b64encode, b"abc", bad=1))
print(run(base64.b64encode, b"abc", None, None))
print(run(base64.b64decode, b"YWJj", None, False, None))
print(run(base64.b64decode, s=b"YWJj", validate=True))
print(run(base64.b32decode, b"ME======", False, None, None))
print(run(base64.b32hexdecode, b"C4======", False, None))
print(run(base64.b16decode, b"6162", False, 1))
print(run(base64.standard_b64decode, b"YWJj", 1))
print(run(base64.urlsafe_b64decode))
print(run(base64.encodebytes, s=b"abc"))
print(run(base64.decodebytes, s=b"YWJj"))
print(run(base64.b85decode, b="Xk~0{Zv"))
print(run(base64.a85decode, b=b"87cURD]i,\"Ebo80"))

orig = base64.b64encode
base64.b64encode = lambda s, altchars=None: b"patched"
print(base64.standard_b64encode(b"abc"), base64.urlsafe_b64encode(b"abc"), base64.b64encode(b"abc"))
base64.b64encode = orig
orig = base64.b64decode
base64.b64decode = lambda s, altchars=None, validate=False: b"patched:" + bytes(s)
print(base64.standard_b64decode(b"abc"), base64.urlsafe_b64decode(b"a-c"))
base64.b64decode = orig
print(base64.standard_b64encode(b"abc"), base64.standard_b64decode(b"YWJj"))


class FakeBinascii:
    Error = binascii.Error

    @staticmethod
    def b2a_base64(data, *, newline=True):
        return b"fake-b2a:" + bytes(data) + (b"\n" if newline else b"")

    @staticmethod
    def a2b_base64(data, *, strict_mode=False):
        return b"fake-a2b:" + bytes(data) + (b"S" if strict_mode else b"")

    @staticmethod
    def hexlify(data):
        return b"fake-hex:" + bytes(data)

    @staticmethod
    def unhexlify(data):
        return b"fake-unhex:" + bytes(data)


real_binascii = base64.binascii
base64.binascii = FakeBinascii
try:
    print(base64.b64encode(b"x"), base64.b64decode(b"y", validate=True), base64.encodebytes(b"z"))
    print(base64.b16encode(b"q"), base64.b16decode(b"AB"), base64.decodebytes(b"w"))
    out = io.BytesIO()
    base64.encode(io.BytesIO(b"abc"), out)
    print(out.getvalue())
finally:
    base64.binascii = real_binascii
print(base64.b64encode(b"x"))

# binascii.Error is the exception class for malformed input
for fn, arg in [
    (base64.b64decode, b"Y"),
    (base64.b32decode, b"M"),
    (base64.b32decode, b"ME======X"),
    (base64.b16decode, b"zz"),
    (base64.b16decode, b"6"),
    (base64.decodebytes, b"Y"),
]:
    try:
        fn(arg)
    except binascii.Error as e:
        print("binascii.Error", fn.__name__, e)
    except Exception as e:
        print("other", fn.__name__, type(e).__name__, e)

print(isinstance(base64.b64encode(b"x"), bytes), type(base64.b32encode(bytearray(b"x"))).__name__)
print(base64.b64decode(b"\xff" if False else b"YQ=="), type(base64.b64decode(b"YQ==")).__name__)
print(sorted(n for n in dir(base64) if not n.startswith("__") and not n.startswith("_")))

# ---------------------------------------------------------------------------
# 10. main()
# ---------------------------------------------------------------------------
import os
import tempfile


class Stream:
    """Minimal text stream with a binary `.buffer`."""

    def __init__(self, data=b""):
        self.buffer = io.BytesIO(data)
        self.text = []

    def write(self, s):
        self.text.append(s)
        return len(s)

    def flush(self):
        pass


def run_main(argv, stdin_bytes=b""):
    saved = (sys.argv, sys.stdout, sys.stderr, sys.stdin)
    out = Stream()
    err = Stream()
    sys.argv = argv
    sys.stdout = out
    sys.stderr = err
    sys.stdin = Stream(stdin_bytes)
    try:
        try:
            result = base64.main()
        except SystemExit as e:
            result = ("exit", e.code)
        except Exception as e:
            result = (type(e).__name__,)
        return result, "".join(out.text), out.buffer.getvalue(), "".join(err.text), err.buffer.getvalue()
    finally:
        sys.argv, sys.stdout, sys.stderr, sys.stdin = saved


with tempfile.TemporaryDirectory() as tmp:
    path = os.path.join(tmp, "data.bin")
    with open(path, "wb") as f:
        f.write(bytes(range(100)))
    enc_path = os.path.join(tmp, "data.b64")
    with open(enc_path, "wb") as f:
        f.write(base64.encodebytes(bytes(range(100))))
    print(run_main(["prog", "-h"]))
    print(run_main(["prog", "-x"]))
    print(run_main(["prog", "-e", path]))
    print(run_main(["prog", path]))
    print(run_main(["prog", "-d", enc_path]))
    print(run_main(["prog", "-u", enc_path]))
    print(run_main(["prog", "-e", "-d", enc_path]))
    print(run_main(["prog", "-d", "-e", path]))
    print(run_main(["prog"], b"stdin data"))
    print(run_main(["prog", "-"], b"stdin data"))
    print(run_main(["prog", "-d", "-"], b"c3RkaW4gZGF0YQ==\n"))
    print(run_main(["prog", os.path.join(tmp, "missing")])[0])
