import array
import base64
import binascii
import io
import sys


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, str(e))


def expect_exc_type(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return type(e).__name__


# 1. Module metadata and __all__
print("in sys.modules:", "base64" in sys.modules)
print("__all__:", base64.__all__)
print("constants:", base64.MAXLINESIZE, base64.MAXBINSIZE, base64.bytes_types)
for name in base64.__all__:
    assert hasattr(base64, name), name
print("all exported ok")

all_bytes = bytes(range(256))


# 2. b64encode / b64decode (altchars, validate, ASCII str, errors)
for sample in [b"", b"a", b"ab", b"abc", b"abcd", b"\xfb\xff\xfe", all_bytes]:
    enc = base64.b64encode(sample)
    assert base64.b64decode(enc) == sample
    assert base64.b64decode(enc, validate=True) == sample
    assert base64.b64decode(enc.decode("ascii")) == sample
    assert base64.b64decode(enc.decode("ascii"), validate=True) == sample
    assert base64.b64encode(bytearray(sample)) == enc
    assert base64.b64decode(bytearray(enc)) == sample
print("b64 roundtrips ok")

print("b64 array:", base64.b64encode(array.array("B", [1, 2, 3])))
print("b64 array H:", base64.b64encode(array.array("H", [1, 2])))

alt_sample = b"\xfb\xff\xfe\xfc\xfd"
enc_std = base64.b64encode(alt_sample)
enc_alt = base64.b64encode(alt_sample, b"-_")
enc_alt2 = base64.b64encode(alt_sample, bytearray(b"!."))
print("b64 altchars:", enc_std, enc_alt, enc_alt2)
print(
    "b64 altchars decode:",
    base64.b64decode(enc_alt, b"-_") == alt_sample,
    base64.b64decode(enc_alt.decode("ascii"), "-_") == alt_sample,
    base64.b64decode(enc_alt, b"-_", validate=True) == alt_sample,
    base64.b64decode(enc_alt2, b"!.", validate=True) == alt_sample,
)
print(
    "b64 validate:",
    base64.b64decode(b"YQ==!@#", validate=False),
    expect_exc(base64.b64decode, b"YQ==!@#", validate=True),
    expect_exc(base64.b64decode, b"Y Q = =", validate=True),
    base64.b64decode(b"Y Q = =", validate=False),
    expect_exc(base64.b64decode, b"YQ=", validate=False),
    expect_exc(base64.b64decode, b"YQ=", validate=True),
)
print(
    "b64 errors:",
    expect_exc_type(base64.b64encode, "abc"),
    expect_exc(base64.b64encode, b"abc", b"-"),
    expect_exc_type(base64.b64encode, b"abc", "-_"),
    expect_exc(base64.b64decode, "\u00e9"),
    expect_exc_type(base64.b64decode, 123),
    expect_exc(base64.b64decode, b"YQ==", b"-"),
    expect_exc(base64.b64decode, b"YQ==", "\u00e9\u00e9"),
)


# 3. standard_b64encode / standard_b64decode & urlsafe_b64encode / urlsafe_b64decode
print(
    "standard_b64:",
    base64.standard_b64encode(alt_sample),
    base64.standard_b64decode(base64.standard_b64encode(alt_sample)) == alt_sample,
    base64.standard_b64decode(base64.standard_b64encode(alt_sample).decode("ascii")) == alt_sample,
)
url_enc = base64.urlsafe_b64encode(alt_sample)
print(
    "urlsafe_b64:",
    url_enc,
    b"+" not in url_enc and b"/" not in url_enc,
    base64.urlsafe_b64decode(url_enc) == alt_sample,
    base64.urlsafe_b64decode(url_enc.decode("ascii")) == alt_sample,
    base64.urlsafe_b64decode(all_bytes_url := base64.urlsafe_b64encode(all_bytes)) == all_bytes,
)


# 4. b32encode / b32decode & b32hexencode / b32hexdecode
rfc_vectors_b32 = [
    (b"", b""),
    (b"f", b"MY======"),
    (b"fo", b"MZXQ===="),
    (b"foo", b"MZXW6==="),
    (b"foob", b"MZXW6YQ="),
    (b"fooba", b"MZXW6YTB"),
    (b"foobar", b"MZXW6YTBOI======"),
]
for raw, expected in rfc_vectors_b32:
    assert base64.b32encode(raw) == expected
    assert base64.b32decode(expected) == raw
    assert base64.b32decode(expected.decode("ascii")) == raw
    assert base64.b32decode(expected.lower(), casefold=True) == raw
print("b32 RFC 4648 ok")

assert base64.b32decode(base64.b32encode(all_bytes)) == all_bytes
print("b32 array:", base64.b32encode(array.array("B", [1, 2, 3])))
print("b32 bytearray:", base64.b32encode(bytearray(b"foo")))
print(
    "b32 map01:",
    base64.b32decode(b"01======", map01=b"L"),
    base64.b32decode("01======", map01="I"),
    base64.b32decode("01======", map01=bytearray(b"L")),
    expect_exc(base64.b32decode, b"01======"),
    expect_exc(base64.b32decode, b"mzxw6===", casefold=False),
    expect_exc(base64.b32decode, b"MZXW6=="),
    expect_exc(base64.b32decode, b"MZXW6==="[:7]),
    expect_exc(base64.b32decode, b"MZXW6==!"),
    expect_exc(base64.b32decode, "\u00e9" * 8),
    expect_exc_type(base64.b32encode, "foo"),
    expect_exc_type(base64.b32decode, 123),
)

rfc_vectors_b32hex = [
    (b"", b""),
    (b"f", b"CO======"),
    (b"fo", b"CPNG===="),
    (b"foo", b"CPNMU==="),
    (b"foob", b"CPNMUOG="),
    (b"fooba", b"CPNMUOJ1"),
    (b"foobar", b"CPNMUOJ1E8======"),
]
for raw, expected in rfc_vectors_b32hex:
    assert base64.b32hexencode(raw) == expected
    assert base64.b32hexdecode(expected) == raw
    assert base64.b32hexdecode(expected.decode("ascii")) == raw
    assert base64.b32hexdecode(expected.lower(), casefold=True) == raw
assert base64.b32hexdecode(base64.b32hexencode(all_bytes)) == all_bytes
print("b32hex RFC 4648 ok")
print(
    "b32hex errors:",
    expect_exc(base64.b32hexdecode, b"cpnmu===", casefold=False),
    expect_exc(base64.b32hexdecode, b"WXYZ===="),
)


# 5. b16encode / b16decode
rfc_vectors_b16 = [
    (b"", b""),
    (b"f", b"66"),
    (b"fo", b"666F"),
    (b"foo", b"666F6F"),
    (b"foob", b"666F6F62"),
    (b"fooba", b"666F6F6261"),
    (b"foobar", b"666F6F626172"),
]
for raw, expected in rfc_vectors_b16:
    assert base64.b16encode(raw) == expected
    assert base64.b16decode(expected) == raw
    assert base64.b16decode(expected.decode("ascii")) == raw
    assert base64.b16decode(expected.lower(), casefold=True) == raw
assert base64.b16decode(base64.b16encode(all_bytes)) == all_bytes
print("b16 array:", base64.b16encode(array.array("B", [0xAB, 0xCD])))
print(
    "b16 errors:",
    expect_exc(base64.b16decode, b"abcd", casefold=False),
    expect_exc(base64.b16decode, "abcd", casefold=False),
    expect_exc(base64.b16decode, b"ABC"),
    expect_exc(base64.b16decode, b"GG"),
    expect_exc(base64.b16decode, "\u00e9\u00e9"),
    expect_exc_type(base64.b16encode, "abc"),
)


# 6. b85encode / b85decode
for raw in [b"", b"a", b"ab", b"abc", b"abcd", b"hello", b"\x00" * 5, b"\xff" * 5, all_bytes]:
    enc = base64.b85encode(raw)
    assert base64.b85decode(enc) == raw
    assert base64.b85decode(enc.decode("ascii")) == raw
    enc_pad = base64.b85encode(raw, pad=True)
    padded_len = len(raw) + ((-len(raw)) % 4)
    assert len(enc_pad) == (padded_len // 4) * 5
    assert base64.b85decode(enc_pad)[: len(raw)] == raw
print("b85 vectors:", base64.b85encode(b"hello"), base64.b85encode(b"hello", pad=True))
print("b85 bytearray/array:", base64.b85encode(bytearray(b"hello")), base64.b85encode(array.array("B", b"hello")))
print(
    "b85 errors:",
    expect_exc(base64.b85decode, b"bad space"),
    expect_exc(base64.b85decode, b"~~~~~"),
    expect_exc(base64.b85decode, "\u00e9"),
    expect_exc_type(base64.b85encode, "hello"),
    expect_exc_type(base64.b85decode, 123),
)


# 7. a85encode / a85decode
for raw in [
    b"",
    b"a",
    b"ab",
    b"abc",
    b"abcd",
    b"\x00",
    b"\x00\x00",
    b"\x00\x00\x00",
    b"\x00\x00\x00\x00",
    b"\x00\x00\x00\x00\x00",
    b"    ",
    b"    \x00\x00\x00\x00hello   ",
    all_bytes,
]:
    for foldspaces in (False, True):
        for adobe in (False, True):
            for wrapcol in (0, 1, 2, 7, 76):
                enc = base64.a85encode(
                    raw, foldspaces=foldspaces, wrapcol=wrapcol, pad=False, adobe=adobe
                )
                dec = base64.a85decode(enc, foldspaces=foldspaces, adobe=adobe)
                assert dec == raw, (raw[:10], foldspaces, adobe, wrapcol, enc, dec)
print("a85 full matrix ok")

print(
    "a85 specifics:",
    base64.a85encode(b"\x00\x00\x00\x00"),
    base64.a85encode(b"\x00\x00", pad=False),
    base64.a85encode(b"\x00\x00", pad=True),
    base64.a85encode(b"    ", foldspaces=False),
    base64.a85encode(b"    ", foldspaces=True),
    base64.a85encode(b"    \x00\x00\x00\x00", foldspaces=True, adobe=True, wrapcol=4),
    base64.a85decode(b"<~~>", adobe=True),
    base64.a85decode(b"~>", adobe=True),
    base64.a85decode("BOu!rDZ", ignorechars=b""),
    base64.a85decode(b"BOu!_rDZ", ignorechars=b"_"),
)
print(
    "a85 errors:",
    expect_exc(base64.a85decode, b"abc", adobe=True),
    expect_exc(base64.a85decode, b"y", foldspaces=False),
    expect_exc(base64.a85decode, b"!z"),
    expect_exc(base64.a85decode, b"!y", foldspaces=True),
    expect_exc(base64.a85decode, b's8W-"'),
    expect_exc(base64.a85decode, b"~"),
    expect_exc(base64.a85decode, b"BOu! rDZ", ignorechars=b""),
    expect_exc(base64.a85decode, "\u00e9"),
    expect_exc_type(base64.a85encode, "hello"),
    expect_exc_type(base64.a85decode, 123),
)


# 8. encodebytes / decodebytes & encode / decode
long_data = bytes((i * 31) & 0xFF for i in range(150))
eb = base64.encodebytes(long_data)
print("encodebytes lines:", [len(line) for line in eb.splitlines()])
print("decodebytes roundtrip:", base64.decodebytes(eb) == long_data)
print(
    "encodebytes empty/short:",
    base64.encodebytes(b""),
    base64.encodebytes(bytearray(b"abc")),
    base64.encodebytes(array.array("B", [65, 66, 67])),
    base64.decodebytes(bytearray(b"YWJj\n")),
    base64.decodebytes(array.array("B", b"YWJj\n")),
)
print(
    "encodebytes/decodebytes errors:",
    expect_exc(base64.encodebytes, "abc"),
    expect_exc(base64.decodebytes, "YWJj\n"),
    expect_exc(base64.encodebytes, array.array("H", [1, 2])),
    expect_exc(base64.decodebytes, array.array("H", [1, 2])),
)

src = io.BytesIO(long_data)
enc_buf = io.BytesIO()
base64.encode(src, enc_buf)
assert enc_buf.getvalue() == eb
dec_buf = io.BytesIO()
base64.decode(io.BytesIO(enc_buf.getvalue()), dec_buf)
assert dec_buf.getvalue() == long_data
print("encode/decode BytesIO ok")
