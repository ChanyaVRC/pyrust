import array
import binascii
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
        return (type(e).__name__, isinstance(e, binascii.Error), isinstance(e, ValueError))


# 1. Module metadata and exception hierarchy
print("in sys.modules:", "binascii" in sys.modules)
print("Error hierarchy:", issubclass(binascii.Error, ValueError), issubclass(binascii.Error, Exception))
print(
    "Incomplete hierarchy:",
    issubclass(binascii.Incomplete, Exception),
    issubclass(binascii.Incomplete, ValueError),
)
print("exc modules:", binascii.Error.__module__, binascii.Incomplete.__module__)
print("distinct aliases:", binascii.hexlify is binascii.b2a_hex, binascii.unhexlify is binascii.a2b_hex)
for name in [
    "Error",
    "Incomplete",
    "hexlify",
    "b2a_hex",
    "unhexlify",
    "a2b_hex",
    "b2a_base64",
    "a2b_base64",
    "crc32",
    "crc_hqx",
    "b2a_uu",
    "a2b_uu",
    "b2a_qp",
    "a2b_qp",
]:
    assert hasattr(binascii, name), name
print("public attrs ok")


# 2. hexlify / b2a_hex
for fn in (binascii.hexlify, binascii.b2a_hex):
    print("hex basic:", fn(b""), fn(b"\x00\x01\x0f\x10\xab\xff"))
    print("hex bytearray:", fn(bytearray(b"abc")))
    print("hex array:", fn(array.array("B", [0, 15, 255])))
    print("hex sep str:", fn(b"\xb9\x01\xef", ":"))
    print("hex sep bytes:", fn(b"\xb9\x01\xef", b"_", 2))
    print("hex sep neg:", fn(b"\x00\x01\x02\x03\x04", ":", -2))
    print("hex sep pos:", fn(b"\x00\x01\x02\x03\x04", ":", 2))
    print("hex sep zero:", fn(b"\x00\x01\x02\x03\x04", ":", 0))
    print("hex sep empty data:", fn(b"", ":"))
    print("hex kwargs:", fn(data=b"abc"), fn(b"abc", sep=":", bytes_per_sep=2))
    print("hex bps only:", fn(b"abc", bytes_per_sep=2))
    print("hex latin1 sep:", fn(b"ab", "\xff"), fn(b"ab", b"\xff"))
    print("hex errors:",
        expect_exc(fn, "abc"),
        expect_exc(fn, b"abc", ""),
        expect_exc(fn, b"abc", "::"),
        expect_exc(fn, b"abc", "\u0100"),
        expect_exc(fn, b"abc", 1),
        expect_exc(fn, b"abc", bytearray(b":")),
        expect_exc(fn, b"abc", ":", "1"),
        expect_exc(fn, b"abc", ":", 1 << 40),
        expect_exc(fn, b"abc", None),
    )


# 3. unhexlify / a2b_hex
for fn in (binascii.unhexlify, binascii.a2b_hex):
    print("unhex basic:", fn(b""), fn(b"00010f10abff"), fn(b"ABCDEF"), fn("0aBcDe"))
    print("unhex bytearray:", fn(bytearray(b"616263")))
    print("unhex array:", fn(array.array("B", b"6162")))
    print("unhex errors:",
        expect_exc(fn, b"a"),
        expect_exc(fn, b"zz"),
        expect_exc(fn, b"61 62"),
        expect_exc(fn, "616"),
        expect_exc(fn, "\u00e9\u00e9"),
        expect_exc(fn, 123),
        expect_exc_type(fn, b"a"),
        expect_exc_type(fn, b"zz"),
    )
    try:
        fn(hexstr=b"61")
        print("unhex kw unexpected ok")
    except TypeError:
        print("unhex kw rejected: TypeError")


# 4. b2a_base64
print("b2a_base64 basic:",
    binascii.b2a_base64(b""),
    binascii.b2a_base64(b"", newline=False),
    binascii.b2a_base64(b"a"),
    binascii.b2a_base64(b"a", newline=False),
    binascii.b2a_base64(b"ab"),
    binascii.b2a_base64(b"abc"),
    binascii.b2a_base64(bytearray(b"hello"), newline=False),
    binascii.b2a_base64(array.array("B", [1, 2, 3]), newline=False),
)


class CustomBool:
    def __init__(self, v):
        self.v = v

    def __bool__(self):
        return self.v


print("b2a_base64 custom bool:",
    binascii.b2a_base64(b"a", newline=CustomBool(False)),
    binascii.b2a_base64(b"a", newline=CustomBool(True)),
)
print("b2a_base64 str err:", expect_exc(binascii.b2a_base64, "abc"))
try:
    binascii.b2a_base64(b"abc", False)
    print("b2a_base64 pos2 unexpected ok")
except TypeError:
    print("b2a_base64 pos2 rejected: TypeError")


# 5. a2b_base64 (roundtrip + strict vs non-strict matrix)
all_bytes = bytes(range(256))
b64_all = binascii.b2a_base64(all_bytes, newline=False)
print("b64 all_bytes roundtrip:", binascii.a2b_base64(b64_all) == all_bytes)
print("b64 all_bytes strict:", binascii.a2b_base64(b64_all, strict_mode=True) == all_bytes)
print("b64 str:", binascii.a2b_base64("YWJj"), binascii.a2b_base64("YWJj", strict_mode=True))
print("b64 bytearray:", binascii.a2b_base64(bytearray(b"YWJj")))
print("b64 non-ascii:", expect_exc(binascii.a2b_base64, "\u00e9"))
print("b64 bad type:", expect_exc(binascii.a2b_base64, 123))

a2b_cases = [
    b"",
    b"=",
    b"==",
    b"====",
    b"YQ==",
    b"YQ=",
    b"YQ",
    b"Y",
    b"YQ==extra",
    b"YQ==\n",
    b"YQ== ",
    b"Y Q = =",
    b"!@#YQ==$%^",
    b"==YQ==",
    b"YQ===",
    b"Y=Q==",
    b"Y=Q=",
    b"Y==Q==",
    b"YR==",
    b"YWJj",
    b"YWJ=",
    b"YWJ==",
    b"YWJ",
    b"YW=J=",
    b"YWJ=extra",
    b"YQ=a",
    b"YQ=a=",
    b"Y=Q",
    b"YQ=Q",
    b"===Y",
    b"Y===",
    b"YW==",
    b"YW===",
    b"YQ= = ",
    b"YQ= =",
    b"YQ== =",
    b"Y=W=J=j=",
    b"YWJjY=",
    b"YWJjYQ=",
    b"YWJjYQ==",
    b"YWJjYWJ=",
    b"YWJjYWJj",
    b"YQ==YWJj",
    b"===YQ==",
]
for c in a2b_cases:
    for sm in (False, True):
        print("a2b_base64", repr(c), sm, "->", expect_exc(binascii.a2b_base64, c, strict_mode=sm))

# Trailing '=' after a complete 4-char quad: CPython 3.12.4 (GH-118761) tightened
# strict_mode=True here compared to 3.12.3; test non-strict parity across all 3.12.x.
for c in (b"YWJj=", b"YWJj==", b"YWJj=a", b"YWJj==a"):
    print("a2b_base64 non-strict", repr(c), "->", expect_exc(binascii.a2b_base64, c, strict_mode=False))


# 6. crc32 and crc_hqx
class CustomIndex:
    def __init__(self, v):
        self.v = v

    def __index__(self):
        return self.v


c_empty = binascii.crc32(b"")
c_hw = binascii.crc32(b"hello world")
c_chained = binascii.crc32(b" world", binascii.crc32(b"hello"))
c_all = binascii.crc32(all_bytes)
print("crc32:", c_empty, c_hw, c_chained == c_hw, c_all)
print("crc32 init variants:",
    binascii.crc32(b"abc", -1),
    binascii.crc32(b"abc", 0x1_0000_0000),
    binascii.crc32(b"abc", (1 << 100) + 5),
    binascii.crc32(b"abc", -((1 << 100) + 5)),
    binascii.crc32(b"abc", True),
    binascii.crc32(b"abc", CustomIndex(42)),
    binascii.crc32(bytearray(b"hello world")) == c_hw,
    binascii.crc32(array.array("B", b"hello world")) == c_hw,
)
print("crc32 range:", 0 <= binascii.crc32(b"\xff\xfe\xfd") <= 0xFFFFFFFF)
print("crc32 str err:", expect_exc(binascii.crc32, "abc"))
try:
    binascii.crc32(b"abc", value=0)
    print("crc32 kw unexpected ok")
except TypeError:
    print("crc32 kw rejected: TypeError")

h_empty = binascii.crc_hqx(b"", 0)
h_hw = binascii.crc_hqx(b"hello world", 0)
h_chained = binascii.crc_hqx(b" world", binascii.crc_hqx(b"hello", 0))
h_all = binascii.crc_hqx(all_bytes, 0)
print("crc_hqx:", h_empty, h_hw, h_chained == h_hw, h_all)
print("crc_hqx mask:", binascii.crc_hqx(b"abc", 0x1FFFF), binascii.crc_hqx(b"abc", -1))
try:
    binascii.crc_hqx(b"abc")
    print("crc_hqx 1arg unexpected ok")
except TypeError:
    print("crc_hqx 1arg rejected: TypeError")


# 7. b2a_uu / a2b_uu
print("uu basic:",
    binascii.b2a_uu(b""),
    binascii.b2a_uu(b"", backtick=True),
    binascii.b2a_uu(b"Cat"),
    binascii.b2a_uu(b"\x00\x00\x00"),
    binascii.b2a_uu(b"\x00\x00\x00", backtick=True),
)
for n in (0, 1, 2, 3, 4, 15, 44, 45):
    sample = bytes((i * 17 + 3) & 0xFF for i in range(n))
    for bt in (False, True):
        enc = binascii.b2a_uu(sample, backtick=bt)
        dec = binascii.a2b_uu(enc)
        assert dec == sample, (n, bt, enc, dec)
print("uu roundtrips ok")
print("uu 46 err:", expect_exc(binascii.b2a_uu, b"a" * 46))
print("uu decode edge:",
    binascii.a2b_uu(b"#0V%T\n"),
    binascii.a2b_uu("#0V%T\n"),
    binascii.a2b_uu(b" \n"),
    binascii.a2b_uu(b"`\n"),
    binascii.a2b_uu(b""),
    binascii.a2b_uu(b"#0V%T   \n"),
    expect_exc(binascii.a2b_uu, b"#0V%Txxxx\n"),
    expect_exc(binascii.a2b_uu, b"!0V%T\n"),
)


# 8. b2a_qp / a2b_qp
qp_samples = [
    (b"hello world\t\n", {}),
    (b"hello world\t\n", {"quotetabs": True}),
    (b"hello world\r\n", {"istext": False}),
    (b"hello world", {"header": True}),
    (b"a_b c", {"header": True}),
    (b"=3D_\x00\xff", {}),
    (b"a" * 75, {}),
    (b"a" * 76, {}),
    (b"a" * 80, {}),
    (b"a " * 40, {}),
    (b"hello \r\nworld\t", {}),
    (b"hello \r\nworld\t", {"istext": False}),
    (b".hello\n.world", {}),
]
for data, kw in qp_samples:
    enc = binascii.b2a_qp(data, **kw)
    dec = binascii.a2b_qp(enc, header=kw.get("header", False))
    print("qp", repr(data[:20]), len(data), kw, "->", repr(enc), "rt:", dec == data)

print("a2b_qp edges:",
    binascii.a2b_qp(b"=61=6a=6A=ZZ=1"),
    binascii.a2b_qp(b"abc=\r\ndef=\nghi=   \njkl"),
    binascii.a2b_qp("=61=62"),
    binascii.a2b_qp(b"hello_world=3D=0A", header=True),
)


# 9. Native module surface (issue #3308): names, metadata, exceptions
print("dir:", sorted(n for n in dir(binascii) if not n.startswith("_")))
print("module doc:", binascii.__doc__, binascii.__name__, hasattr(binascii, "__all__"))
print("Error meta:", binascii.Error.__module__, binascii.Error.__qualname__, binascii.Error.__name__, binascii.Error.__doc__)
print("Error mro:", [c.__name__ for c in binascii.Error.__mro__])
print("Incomplete meta:", binascii.Incomplete.__module__, binascii.Incomplete.__qualname__, binascii.Incomplete.__doc__)
print("Incomplete mro:", [c.__name__ for c in binascii.Incomplete.__mro__])
print("exc repr:", repr(binascii.Error("x")), str(binascii.Error("x")), repr(binascii.Incomplete()))
print("exc str type:", type(binascii.Error("x")).__name__, str(binascii.Error()), binascii.Error("a", "b").args)
for fname in [
    "b2a_hex", "hexlify", "a2b_hex", "unhexlify", "b2a_base64", "a2b_base64",
    "crc32", "crc_hqx", "b2a_uu", "a2b_uu", "b2a_qp", "a2b_qp",
]:
    fobj = getattr(binascii, fname)
    print("func meta:", fname, fobj.__name__, fobj.__module__, type(fobj).__name__, callable(fobj))
print("not aliased:", binascii.hexlify is binascii.b2a_hex, binascii.unhexlify is binascii.a2b_hex)
print("no private helpers:", [n for n in vars(binascii) if n.startswith("_") and not n.startswith("__")])

caught = []
for label, fn, args, kwargs in [
    ("hex noargs", binascii.b2a_hex, (), {}),
    ("a2b_hex kw", binascii.a2b_hex, (), {"hexstr": b"61"}),
    ("b2a_base64 newline positional", binascii.b2a_base64, (b"a", True), {}),
    ("a2b_base64 strict positional", binascii.a2b_base64, (b"YQ==", True), {}),
    ("b2a_uu backtick positional", binascii.b2a_uu, (b"a", True), {}),
    ("crc32 kw", binascii.crc32, (b"a",), {"crc": 1}),
    ("crc_hqx one arg", binascii.crc_hqx, (b"a",), {}),
    ("crc_hqx kw", binascii.crc_hqx, (b"a",), {"crc": 1}),
    ("a2b_qp too many", binascii.a2b_qp, (b"a", True, 1), {}),
    ("b2a_qp bad kw", binascii.b2a_qp, (b"a",), {"bogus": 1}),
]:
    try:
        fn(*args, **kwargs)
        caught.append((label, "OK"))
    except Exception as exc:
        caught.append((label, type(exc).__name__))
print("arg errors:", caught)

print("kw ok:", binascii.a2b_qp(b"=41_", header=True), binascii.b2a_qp(b"a b\n", quotetabs=True, istext=False, header=True))
print("kw ok2:", binascii.b2a_base64(b"a", newline=False), binascii.a2b_base64(b"YQ==", strict_mode=False), binascii.b2a_uu(b"a", backtick=True))

# Argument conversion order and type messages
print("conv:",
    expect_exc(binascii.b2a_hex, "str", ":", "x"),
    expect_exc(binascii.b2a_hex, b"ab", sep=None),
    expect_exc(binascii.b2a_hex, b"ab", ":", 2**31),
    expect_exc(binascii.b2a_hex, b"ab", ":", -(2**31)),
    expect_exc(binascii.b2a_hex, b"ab", ":", 2**100),
    expect_exc(binascii.crc32, b"a", None),
    expect_exc(binascii.crc32, b"a", 1.0),
    expect_exc(binascii.crc32, b"a", 2**70 + 5),
    expect_exc(binascii.crc32, b"a", -(2**70) - 5),
    expect_exc(binascii.crc32, b"a", True),
    expect_exc(binascii.crc_hqx, b"a", 2**70 + 5),
    expect_exc(binascii.crc_hqx, b"a", "x"),
)


class _Idx:
    def __index__(self):
        return 3


class _Str(str):
    pass


class _Bytes(bytes):
    pass


print("subclasses:",
    expect_exc(binascii.a2b_hex, _Str("6162")),
    expect_exc(binascii.b2a_hex, _Bytes(b"ab")),
    expect_exc(binascii.b2a_hex, _Bytes(b"ab"), _Str(":"), 1),
    expect_exc(binascii.b2a_hex, b"abcdef", ":", _Idx()),
    expect_exc(binascii.crc32, b"a", _Idx()),
)
print("non-buffer inputs:",
    expect_exc(binascii.b2a_hex, None),
    expect_exc(binascii.b2a_hex, [1]),
    expect_exc(binascii.b2a_base64, 5),
    expect_exc(binascii.b2a_uu, "str"),
    expect_exc(binascii.b2a_qp, 1.5),
    expect_exc(binascii.crc32, "str"),
    expect_exc(binascii.a2b_hex, 5),
    expect_exc(binascii.a2b_base64, None),
    expect_exc(binascii.a2b_uu, None),
    expect_exc(binascii.a2b_qp, 1.5),
    expect_exc(binascii.a2b_hex, "\u00e9"),
    expect_exc(binascii.a2b_base64, "\u00e9"),
)
released = memoryview(bytearray(b"abc"))
released.release()
print("released memoryview:", expect_exc(binascii.b2a_hex, released))
print("array inputs:",
    binascii.b2a_hex(array.array("i", [1, 2])),
    binascii.crc32(array.array("H", [1, 2])),
    binascii.b2a_base64(array.array("B", [1, 2, 3]), newline=0),
)
print("truthy flags:", binascii.b2a_base64(b"a", newline=[]), binascii.b2a_qp(b"a b\n", 1, 0, 1))
print("exc isinstance:",
    expect_exc_type(binascii.a2b_hex, b"abc"),
    expect_exc_type(binascii.a2b_base64, b"Y", strict_mode=True),
    expect_exc_type(binascii.b2a_uu, b"a" * 46),
)
