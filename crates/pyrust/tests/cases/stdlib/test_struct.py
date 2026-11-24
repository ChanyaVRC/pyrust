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
