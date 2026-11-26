import math
import struct
import sys
from struct import Struct, calcsize, error, iter_unpack, pack, pack_into, unpack, unpack_from

# 1. Module metadata and exports
print("in sys.modules:", "struct" in sys.modules)
print("__all__:", struct.__all__)
print("dir public:", [n for n in dir(struct) if not n.startswith("__")])
print("error:", issubclass(error, Exception), error.__module__)
print("clearcache:", struct._clearcache())

# 2. calcsize across byte-order prefixes, native alignment, zero-repeat alignment, whitespace, and bytes format
for fmt in [
    "",
    "x",
    "3x",
    "0x",
    "b",
    "bi",
    "ib",
    "bi0i",
    "bh",
    "hb",
    "bq",
    "qb",
    "b0q",
    "i0q",
    "c0d",
    "bd",
    "db",
    "be",
    "eb",
    "bP",
    "Pb",
    "bn",
    "nb",
    "bN",
    "Nb",
    "bl",
    "lb",
    "b 3i",
    " \t\n\r\x0b\x0c 2h ",
    "0i",
    "10s",
    "0s",
    "0p",
    "1p",
    "5p",
    "<bi",
    "=bi",
    ">bi",
    "!bi",
    "@bi",
    b"<i",
    b"bi",
    "<bBhHiIlLqQefd",
    ">bBhHiIlLqQefd",
    "!bBhHiIlLqQefd",
    "=bBhHiIlLqQefd",
]:
    print("calcsize", repr(fmt), "->", calcsize(fmt))

# 3. Standard endianness pack/unpack (<, >, !, =)
for prefix in ["<", ">", "!", "="]:
    packed = pack(
        prefix + "bBhHiIlLqQ",
        -128,
        255,
        -32768,
        65535,
        -2147483648,
        4294967295,
        -123456789,
        3123456789,
        -9223372036854775808,
        18446744073709551615,
    )
    print(prefix, "len:", len(packed), "unpacked:", unpack(prefix + "bBhHiIlLqQ", packed))
    if prefix != "=":
        print(prefix, "hex:", packed.hex())

# 4. Native '@' mode including platform-native l/L, n/N, P, alignment padding
native_packed = pack("@bhiq", -5, 1024, -70000, 1234567890123)
print("native aligned:", len(native_packed), unpack("@bhiq", native_packed))
print("native lL:", unpack("@lL", pack("@lL", -42, 42)))
print("native nNP:", unpack("@nNP", pack("@nNP", -99, 12345, 0x12345678)))
print("native big N/P:", unpack("@NP", pack("@NP", (1 << 63) + 7, (1 << 63) + 9)))

# 5. Pad bytes ('x'), chars ('c'), booleans ('?'), strings ('s', 'p')
print("pad:", pack("<b2xb", 1, 2), unpack("<b2xb", pack("<b2xb", 1, 2)))
print("char:", pack("3c", b"A", b"B", b"C"), unpack("3c", b"ABC"))
print("bool:", pack("????", 0, 1, "", [1]), unpack("????", b"\x00\x01\x02\xff"))


class CustomTruthy:
    def __init__(self, flag):
        self.flag = flag

    def __bool__(self):
        return self.flag


print("bool dunder:", pack("??", CustomTruthy(False), CustomTruthy(True)))

print("0s:", pack("0s", b"abc"), unpack("0s", b""))
print("s:", pack("s", b"ab"), unpack("s", b"a"))
print("5s pad:", pack("5s", b"ab"), unpack("5s", pack("5s", b"ab")))
print("5s trunc:", pack("5s", b"abcdefg"), unpack("5s", pack("5s", b"abcdefg")))
print("s bytearray:", pack("4s", bytearray(b"hi")))

print("1p:", pack("1p", b"abc"), unpack("1p", b"\x05"))
print("5p pad:", pack("5p", b"ab"), unpack("5p", pack("5p", b"ab")))
print("5p trunc:", pack("5p", b"abcdef"), unpack("5p", pack("5p", b"abcdef")))
print("5p clamp:", unpack("5p", b"\x09abcd"))
print("p bytearray:", pack("5p", bytearray(b"hi")))
p300 = pack("300p", b"x" * 299)
print("300p:", len(p300), p300[0], len(unpack("300p", p300)[0]))

# 6. Floating point ('e', 'f', 'd') including binary16 edge cases
for v in [
    0.0,
    -0.0,
    1.0,
    -2.5,
    65504.0,
    65519.99,
    6.103515625e-5,
    5.960464477539063e-8,
    1.0 + 2**-11,
    1.0 + 3 * 2**-11,
    1.0 + 2**-10 + 2**-11,
    2**-25,
    2**-25 * 1.5,
    float("inf"),
    float("-inf"),
]:
    be = pack(">e", v)
    le = pack("<e", v)
    u = unpack("<e", le)[0]
    print("e", repr(v), le.hex(), be.hex(), repr(u), math.copysign(1.0, u))

nan_e = unpack("<e", pack("<e", float("nan")))[0]
print("e nan:", math.isnan(nan_e))

for fmt_ch in ["f", "d"]:
    p_le = pack("<" + fmt_ch, -3.25)
    p_be = pack(">" + fmt_ch, -3.25)
    print(fmt_ch, p_le.hex(), p_be.hex(), unpack("<" + fmt_ch, p_le), unpack(">" + fmt_ch, p_be))
    print(fmt_ch, "inf:", unpack("<" + fmt_ch, pack("<" + fmt_ch, float("inf"))))
    print(fmt_ch, "nan:", math.isnan(unpack("<" + fmt_ch, pack("<" + fmt_ch, float("nan")))[0]))

# 7. __index__ and __float__ protocols
class MyIndex:
    def __index__(self):
        return 7


class MyFloat:
    def __float__(self):
        return 2.5


print("index int:", pack("<i", MyIndex()).hex())
print("float dunder:", pack("<f", MyFloat()).hex())
print("index float:", pack("<f", MyIndex()).hex())
print("bool int:", pack("b", True).hex(), pack("b", False).hex())

# 8. pack_into, unpack_from (bytes & bytearray, positive & negative offsets, kwargs)
buf = bytearray(12)
pack_into("<H", buf, 0, 0x1234)
pack_into(">I", buf, 2, 0xDEADBEEF)
pack_into("<i", buf, -4, -999)
print("pack_into buf:", bytes(buf).hex())
print("unpack_from default:", unpack_from("<H", buf))
print("unpack_from pos:", unpack_from(">I", bytes(buf), 2))
print("unpack_from neg:", unpack_from("<i", buf, -4))
print("unpack_from kw:", unpack_from("<i", buffer=buf, offset=-4))
print("pack_into empty end:", pack_into("", buf, 12))
print("unpack_from empty end:", unpack_from("", buf, 12))

# 9. iter_unpack (bytes & bytearray, empty buffer)
raw_pairs = pack("<HHHH", 10, 20, 30, 40)
print("iter_unpack bytes:", list(iter_unpack("<HH", raw_pairs)))
print("iter_unpack bytearray:", list(iter_unpack("<H", bytearray(raw_pairs))))
print("iter_unpack empty:", list(iter_unpack("<H", b"")))

# 10. Struct class (.format, .size, bytes format, methods, subclassing)
s_str = Struct("<Hi")
s_bytes = Struct(b" <Hi"[1:])
print("Struct str:", s_str.format, type(s_str.format).__name__, s_str.size)
print("Struct bytes:", s_bytes.format, type(s_bytes.format).__name__, s_bytes.size)
packed_s = s_str.pack(7, -100)
print("Struct pack/unpack:", packed_s.hex(), s_str.unpack(packed_s))
ba_s = bytearray(10)
s_str.pack_into(ba_s, 2, 11, 22)
print("Struct pack_into/unpack_from:", s_str.unpack_from(ba_s, 2), s_str.unpack_from(buffer=ba_s, offset=-8))
print("Struct iter_unpack:", list(s_str.iter_unpack(packed_s + packed_s)))


class SubStruct(Struct):
    pass


sub = SubStruct(">h")
print("SubStruct:", sub.format, sub.size, sub.pack(-3).hex(), sub.unpack(sub.pack(-3)))

# 11. Read-only attributes on Struct
try:
    s_str.format = "<i"
    print("format set unexpectedly")
except AttributeError:
    print("format readonly: AttributeError")

try:
    s_str.size = 99
    print("size set unexpectedly")
except AttributeError:
    print("size readonly: AttributeError")

# 12. Error handling (struct.error, TypeError, OverflowError, UnicodeEncodeError)
def check_exc(label, expected_type, fn):
    try:
        fn()
        print("UNEXPECTED OK:", label)
    except expected_type as e:
        print("OK EXC:", label, type(e).__name__)


check_exc("bad char", error, lambda: calcsize("z"))
check_exc("leading space before prefix", error, lambda: calcsize(" <i"))
check_exc("repeat before prefix", error, lambda: calcsize("2<i"))
check_exc("second prefix", error, lambda: calcsize("<i>i"))
check_exc("non-native n", error, lambda: calcsize("<n"))
check_exc("non-native N", error, lambda: calcsize(">N"))
check_exc("non-native P", error, lambda: calcsize("=P"))
check_exc("trailing repeat", error, lambda: calcsize("3"))
check_exc("non-ascii bytes fmt", error, lambda: calcsize(b"\xff"))
check_exc("non-ascii str fmt", UnicodeEncodeError, lambda: calcsize("\u00e9"))
check_exc("bad fmt type", TypeError, lambda: calcsize(123))
check_exc("pack too few", error, lambda: pack("<ii", 1))
check_exc("pack too many", error, lambda: pack("<i", 1, 2))
check_exc("b overflow", error, lambda: pack("b", 128))
check_exc("b underflow", error, lambda: pack("b", -129))
check_exc("B negative", error, lambda: pack("B", -1))
check_exc("B overflow", error, lambda: pack("B", 256))
check_exc("q overflow", error, lambda: pack("<q", 1 << 100))
check_exc("Q negative", error, lambda: pack("<Q", -1))
check_exc("i float", error, lambda: pack("<i", 1.5))
check_exc("i str", error, lambda: pack("<i", "1"))
check_exc("f str", error, lambda: pack("<f", "1"))
check_exc("e overflow", OverflowError, lambda: pack("<e", 65520.0))
check_exc("f overflow", OverflowError, lambda: pack("<f", 1e39))
check_exc("c len 2", error, lambda: pack("c", b"ab"))
check_exc("c str", error, lambda: pack("c", "a"))
check_exc("c bytearray", error, lambda: pack("c", bytearray(b"a")))
check_exc("s str", error, lambda: pack("3s", "abc"))
check_exc("p str", error, lambda: pack("3p", "abc"))
check_exc("unpack short", error, lambda: unpack("<i", b"123"))
check_exc("unpack bad buf", TypeError, lambda: unpack("<i", "1234"))
check_exc("unpack_from oob", error, lambda: unpack_from("<i", b"1234", 1))
check_exc("unpack_from neg oob", error, lambda: unpack_from("<i", b"1234", -5))
check_exc("unpack_from neg partial", error, lambda: unpack_from("<i", b"1234", -2))
check_exc("unpack_from empty oob", error, lambda: unpack_from("", b"1234", 5))
check_exc("pack_into readonly", TypeError, lambda: pack_into("<i", b"1234", 0, 1))
check_exc("pack_into oob", error, lambda: pack_into("<i", bytearray(4), 1, 1))
check_exc("pack_into neg oob", error, lambda: pack_into("<i", bytearray(4), -5, 1))
check_exc("pack_into neg partial", error, lambda: pack_into("<i", bytearray(4), -2, 1))
check_exc("pack_into empty oob", error, lambda: pack_into("", bytearray(4), 5))
check_exc("iter_unpack size 0", error, lambda: iter_unpack("", b"abc"))
check_exc("iter_unpack non-multiple", error, lambda: iter_unpack("<i", b"123"))

# 13. Reload generation isolation: previously imported functions raise their own error class
old_error = struct.error
old_pack = struct.pack
del sys.modules["struct"]
import struct as reloaded_struct

try:
    old_pack("<i", "bad")
except old_error:
    print("old_pack raised old_error: True")
try:
    reloaded_struct.pack("<i", "bad")
except reloaded_struct.error:
    print("new_pack raised new_error: True")

# 14. Module surface: __doc__, __all__ order, hidden iterator class, error identity
print("doc:", struct.__doc__.splitlines()[0])
print("doc mentions error:", "struct.error" in struct.__doc__)
print("no unpack_iterator export:", not hasattr(struct, "unpack_iterator"), "unpack_iterator" not in dir(struct))
print("error bases:", error.__bases__, error.__name__, error.__qualname__, error.__doc__)
print("Struct identity:", Struct.__name__, Struct.__module__, Struct.__doc__ == "Struct(fmt) --> compiled struct object\n\n")
print("error is Exception subclass only:", error.__mro__[1:] == (Exception, BaseException, object))
try:
    pack("<i", "bad")
except error as exc:
    print("raised struct.error:", type(exc) is struct.error, exc.args)


# 15. Top-level call shapes and CPython diagnostics
def show_exc(label, fn):
    try:
        print("OK:", label, repr(fn()))
    except Exception as exc:
        print("EXC:", label, type(exc).__name__, exc)


show_exc("calcsize()", lambda: calcsize())
show_exc("calcsize(a, b)", lambda: calcsize("i", 2))
show_exc("calcsize(format=)", lambda: calcsize(format="i"))
show_exc("pack()", lambda: pack())
show_exc("pack(format=)", lambda: pack(format="i"))
show_exc("pack kw", lambda: pack("i", 1, x=2))
show_exc("pack bad format type", lambda: pack(1))
show_exc("unpack()", lambda: unpack())
show_exc("unpack(fmt)", lambda: unpack("i"))
show_exc("unpack 3 args", lambda: unpack("i", b"1234", 3))
show_exc("unpack kw", lambda: unpack("i", buffer=b"1234"))
show_exc("unpack_from()", lambda: unpack_from())
show_exc("unpack_from(fmt)", lambda: unpack_from("i"))
show_exc("unpack_from kw buffer/offset", lambda: unpack_from("i", buffer=b"12345678", offset=4))
show_exc("unpack_from kw format", lambda: unpack_from(format="i", buffer=b"12345678"))
show_exc("unpack_from bad kw", lambda: unpack_from("i", b"1234", foo=1))
show_exc("unpack_from too many", lambda: unpack_from("i", b"1234", 0, 1))
show_exc("unpack_from dup buffer", lambda: unpack_from("i", b"1234", buffer=b"1234"))
show_exc("unpack_from dup offset", lambda: unpack_from("i", b"12345678", 4, offset=4))
show_exc("unpack_from offset str", lambda: unpack_from("i", b"1234", "a"))
show_exc("unpack_from offset float", lambda: unpack_from("i", b"1234", 1.0))
show_exc("unpack_from offset None", lambda: unpack_from("i", b"1234", None))
show_exc("unpack_from offset huge", lambda: unpack_from("i", bytes(4), 2**70))
show_exc("pack_into()", lambda: pack_into())
show_exc("pack_into(fmt, buf)", lambda: pack_into("i", bytearray(4)))
show_exc("pack_into(fmt)", lambda: pack_into("i"))
show_exc("pack_into kw", lambda: pack_into("i", bytearray(4), 0, 1, x=1))
show_exc("pack_into offset huge", lambda: pack_into("i", bytearray(4), 2**70, 1))
show_exc("pack_into offset str", lambda: pack_into("i", bytearray(4), "a", 1))
show_exc("iter_unpack()", lambda: iter_unpack())
show_exc("iter_unpack(fmt)", lambda: iter_unpack("i"))
show_exc("iter_unpack kw", lambda: iter_unpack("i", buffer=b"1234"))
show_exc("iter_unpack str buffer", lambda: iter_unpack("i", "1234"))
show_exc("_clearcache(1)", lambda: struct._clearcache(1))
show_exc("calcsize huge", lambda: calcsize("99999999999999999999c"))
show_exc("calcsize 2**31-1", lambda: calcsize("2147483647c"))
show_exc("memoryview unpack_from", lambda: unpack_from("<2s", memoryview(b"abcd"), 1))
show_exc("memoryview format", lambda: calcsize(memoryview(b"i")))


class StrSub(str):
    pass


class BytesSub(bytes):
    pass


show_exc("str subclass format", lambda: calcsize(StrSub("<i")))
show_exc("bytes subclass Struct", lambda: Struct(BytesSub(b"<i")).format)

# 16. Struct construction, re-initialisation, and method call shapes
s_i = Struct("<i")
show_exc("Struct()", lambda: Struct())
show_exc("Struct(format=)", lambda: Struct(format="i").format)
show_exc("Struct(x=)", lambda: Struct(x="i"))
show_exc("Struct(a, b)", lambda: Struct("i", 1))
show_exc("Struct(None)", lambda: Struct(None))
show_exc("Struct(bad)", lambda: Struct("z"))
show_exc("Struct.__init__()", lambda: s_i.__init__())
show_exc("Struct reinit", lambda: (s_i.__init__(">h"), s_i.format, s_i.size))
s_i = Struct("<i")
show_exc("s.pack()", lambda: s_i.pack())
show_exc("s.pack(1, 2)", lambda: s_i.pack(1, 2))
show_exc("s.pack kw", lambda: s_i.pack(1, x=1))
show_exc("s.unpack()", lambda: s_i.unpack())
show_exc("s.unpack(a, b)", lambda: s_i.unpack(b"1234", 1))
show_exc("s.unpack kw", lambda: s_i.unpack(buffer=b"1234"))
show_exc("s.unpack short", lambda: s_i.unpack(b"12"))
show_exc("s.unpack_from()", lambda: s_i.unpack_from())
show_exc("s.unpack_from kw", lambda: s_i.unpack_from(buffer=b"12345678", offset=4))
show_exc("s.unpack_from bad kw", lambda: s_i.unpack_from(b"1234", x=4))
show_exc("s.unpack_from too many", lambda: s_i.unpack_from(b"1234", 0, 1))
show_exc("s.unpack_from short", lambda: s_i.unpack_from(b"12"))
show_exc("s.unpack_from past end", lambda: s_i.unpack_from(b"1234", 4))
show_exc("s.unpack_from negative", lambda: s_i.unpack_from(b"12345678", -4))
show_exc("s.pack_into()", lambda: s_i.pack_into())
show_exc("s.pack_into(buf)", lambda: s_i.pack_into(bytearray(4)))
show_exc("s.pack_into ok", lambda: (lambda b: (s_i.pack_into(b, 0, 7), bytes(b)))(bytearray(4)))
show_exc("s.pack_into readonly", lambda: s_i.pack_into(b"1234", 0, 1))
show_exc("s.pack_into short", lambda: s_i.pack_into(bytearray(3), 0, 1))
show_exc("s.pack_into kw", lambda: s_i.pack_into(bytearray(4), 0, 1, x=1))
show_exc("s.pack_into wrong count", lambda: s_i.pack_into(bytearray(4), 0))
show_exc("s.iter_unpack()", lambda: s_i.iter_unpack())
show_exc("s.iter_unpack kw", lambda: s_i.iter_unpack(buffer=b"1234"))
show_exc("s.iter_unpack not multiple", lambda: s_i.iter_unpack(b"123"))
show_exc("empty Struct iter_unpack", lambda: Struct("").iter_unpack(b"123"))


class TwoArgStruct(Struct):
    def __init__(self, fmt, extra=0):
        super().__init__(fmt)
        self.extra = extra


two = TwoArgStruct("<h", 5)
print("subclass custom __init__:", two.format, two.size, two.extra, two.pack(9).hex(), type(two).__name__)
print("subclass kw format:", SubStruct(format="<h").size)
print("Struct repr:", repr(s_i).startswith("<_struct.Struct object at 0x"), type(s_i).__name__)
print("Struct __hash__/eq:", isinstance(hash(s_i), int), s_i == Struct("<i"), s_i == s_i)

# 17. iter_unpack iterator object
it = iter_unpack("<H", b"\x01\x00\x02\x00\x03\x00")
print("iter type:", type(it).__name__, type(it).__module__, repr(it).startswith("<_struct.unpack_iterator object at "))
print("iter is self:", iter(it) is it, it.__length_hint__())
print("iter next:", next(it), it.__length_hint__())
print("iter rest:", list(it), it.__length_hint__())
try:
    next(it)
except StopIteration:
    print("iter exhausted: StopIteration")
try:
    type(it)("<H", b"")
except TypeError as exc:
    print("iter construct:", exc)
print("iter bytearray + memoryview:", list(iter_unpack("<H", bytearray(b"\x01\x00"))), list(iter_unpack("<H", memoryview(b"\x02\x00"))))
print("iter struct pairs:", list(Struct("<2b").iter_unpack(b"\x01\x02\x03\x04")))
for pair in iter_unpack("<bb", b"\x01\x02\x03\x04"):
    print("iter loop:", pair)

# 18. Format cache: _clearcache is safe at any time and does not change results
before = pack("<hh", 1, 2)
print("clearcache before:", struct._clearcache(), before == pack("<hh", 1, 2))
for n in range(120):
    calcsize("<%dh" % (n + 1))
print("cache churn:", pack("<hh", 1, 2) == before, calcsize("<120h"))
print("clearcache after:", struct._clearcache(), unpack("<hh", before))

# 19. Re-imported module keeps one struct.error class (CPython's _struct persists)
print("reload error identity:", reloaded_struct.error is old_error)
try:
    reloaded_struct.pack("<i", "bad")
except old_error:
    print("reloaded pack raises shared error: True")
try:
    old_pack("<i", "bad")
except reloaded_struct.error:
    print("old pack raises shared error: True")
rs = reloaded_struct.Struct("<h")
print("reloaded Struct:", rs.pack(3).hex(), list(reloaded_struct.iter_unpack("<h", b"\x03\x00")))
