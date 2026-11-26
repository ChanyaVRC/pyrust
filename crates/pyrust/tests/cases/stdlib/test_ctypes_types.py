"""CPython 3.12.3 parity tests for ctypes primitive/compound types, memory helpers, and wintypes."""

import ctypes
import _ctypes
import ctypes._endian
import ctypes.wintypes as wt
import struct
import sys

# 1. Core hierarchy and primitive type metadata
prim_names = [
    "c_bool",
    "c_char",
    "c_wchar",
    "c_byte",
    "c_ubyte",
    "c_short",
    "c_ushort",
    "c_int",
    "c_uint",
    "c_long",
    "c_ulong",
    "c_longlong",
    "c_ulonglong",
    "c_size_t",
    "c_ssize_t",
    "c_float",
    "c_double",
    "c_longdouble",
    "c_char_p",
    "c_wchar_p",
    "c_void_p",
    "c_int8",
    "c_uint8",
    "c_int16",
    "c_uint16",
    "c_int32",
    "c_uint32",
    "c_int64",
    "c_uint64",
    "py_object",
]
for name in prim_names:
    cls = getattr(ctypes, name)
    assert issubclass(cls, ctypes._SimpleCData), name
    print(name, ctypes.sizeof(cls), ctypes.alignment(cls))

# 2. Primitive value get/set, wrapping, bool, repr, bytes()
assert ctypes.c_bool().value is False
assert ctypes.c_bool(1).value is True
assert ctypes.c_bool(42).value is True
assert bool(ctypes.c_bool(False)) is False
assert bool(ctypes.c_bool(True)) is True

assert ctypes.c_char().value == b"\x00"
assert ctypes.c_char(b"A").value == b"A"
assert ctypes.c_char(66).value == b"B"
assert bytes(ctypes.c_char(b"Z")) == b"Z"

assert ctypes.c_wchar().value == "\x00"
assert ctypes.c_wchar("X").value == "X"

assert ctypes.c_byte(-128).value == -128
assert ctypes.c_byte(128).value == -128
assert ctypes.c_ubyte(-1).value == 255
assert ctypes.c_short(32768).value == -32768
assert ctypes.c_ushort(-1).value == 65535
assert ctypes.c_int32(0x7FFFFFFF).value == 0x7FFFFFFF
assert ctypes.c_int32(0x80000000).value == -0x80000000
assert ctypes.c_uint32(-1).value == 0xFFFFFFFF
assert ctypes.c_int64(-1234567890123).value == -1234567890123
assert ctypes.c_uint64(-1).value == 0xFFFFFFFFFFFFFFFF
assert ctypes.c_uint64(2**64 - 1).value == 2**64 - 1

f = ctypes.c_float(1.5)
assert f.value == 1.5
d = ctypes.c_double(3.25)
assert d.value == 3.25
assert bytes(d) == struct.pack("=d", 3.25)
assert bytes(ctypes.c_int32(0x01020304)) == struct.pack("=i", 0x01020304)

# c_char_p / c_wchar_p / c_void_p / py_object
cp = ctypes.c_char_p(b"hello")
assert cp.value == b"hello"
assert bool(cp) is True
cp.value = b"world!"
assert cp.value == b"world!"
cp.value = None
assert cp.value is None
assert bool(cp) is False

wp = ctypes.c_wchar_p("hello unicode")
assert wp.value == "hello unicode"
wp.value = None
assert wp.value is None

vp = ctypes.c_void_p()
assert vp.value is None
assert bool(vp) is False
vp = ctypes.c_void_p(0x1234)
assert vp.value == 0x1234
assert bool(vp) is True

sentinel = {"k": [1, 2, 3]}
po = ctypes.py_object(sentinel)
assert po.value is sentinel
assert bool(po) is True
null_po = ctypes.py_object()
assert bool(null_po) is False
try:
    _ = null_po.value
    assert False, "expected ValueError"
except ValueError:
    pass
print("primitives ok")

# 3. Buffer protocol, from_buffer, from_buffer_copy, from_address, resize
ba = bytearray(struct.pack("=i", 0x11223344))
ci = ctypes.c_int32.from_buffer(ba)
assert ci.value == 0x11223344
ci.value = 0x55667788
assert bytes(ba) == struct.pack("=i", 0x55667788)

ci_copy = ctypes.c_int32.from_buffer_copy(bytes(ba))
assert ci_copy.value == 0x55667788
ci_copy.value = 0
assert bytes(ba) == struct.pack("=i", 0x55667788)

try:
    ctypes.c_int32.from_buffer(b"1234")
    assert False, "expected TypeError for immutable bytes"
except TypeError:
    pass

orig = ctypes.c_int32(999)
alias = ctypes.c_int32.from_address(ctypes.addressof(orig))
assert alias.value == 999
alias.value = -777
assert orig.value == -777

buf_mv = memoryview(orig)
assert buf_mv.nbytes == 4
assert bytes(buf_mv) == struct.pack("=i", -777)
buf_mv.release()

r = ctypes.c_int32(10)
ctypes.resize(r, 16)
assert ctypes.sizeof(r) == 16
try:
    ctypes.resize(r, 2)
    assert False, "expected ValueError"
except ValueError:
    pass
print("buffers ok")

# 4. Arrays (c_type * n and n * c_type)
IntArr4 = ctypes.c_int32 * 4
assert (4 * ctypes.c_int32) is IntArr4
assert ctypes.sizeof(IntArr4) == 16
assert ctypes.alignment(IntArr4) == 4
arr = IntArr4(10, 20, 30, 40)
assert len(arr) == 4
assert list(arr) == [10, 20, 30, 40]
assert arr[-1] == 40
assert arr[1:3] == [20, 30]
arr[1:3] = [200, 300]
assert list(arr) == [10, 200, 300, 40]

CharArr = ctypes.c_char * 6
ca = CharArr(b"a", b"b", b"c")
assert ca.value == b"abc"
assert ca.raw == b"abc\x00\x00\x00"
assert ca[:4] == b"abc\x00"
ca.raw = b"hi"
assert ca.raw == b"hic\x00\x00\x00"
ca.value = b"xy\x00z"
assert ca.value == b"xy"
assert ca.raw == b"xy\x00z\x00\x00"
print("arrays ok")

# 5. Structures, padding, alignment, _pack_, nested structs, Union, _anonymous_
class Padded(ctypes.Structure):
    _fields_ = [
        ("a", ctypes.c_byte),
        ("b", ctypes.c_int32),
        ("c", ctypes.c_byte),
    ]

print("Padded:", ctypes.sizeof(Padded), ctypes.alignment(Padded),
      Padded.a.offset, Padded.a.size,
      Padded.b.offset, Padded.b.size,
      Padded.c.offset, Padded.c.size)
p = Padded(1, 0x12345678, 2)
assert (p.a, p.b, p.c) == (1, 0x12345678, 2)
p2 = Padded(b=77, a=-5)
assert (p2.a, p2.b, p2.c) == (-5, 77, 0)

class Packed(ctypes.Structure):
    _pack_ = 1
    _fields_ = [
        ("a", ctypes.c_byte),
        ("b", ctypes.c_int32),
        ("c", ctypes.c_byte),
    ]

print("Packed:", ctypes.sizeof(Packed), ctypes.alignment(Packed),
      Packed.a.offset, Packed.b.offset, Packed.c.offset)
assert ctypes.sizeof(Packed) == 6
assert Packed.b.offset == 1

class Inner(ctypes.Structure):
    _fields_ = [("x", ctypes.c_int32), ("y", ctypes.c_int32)]

class Outer(ctypes.Structure):
    _fields_ = [("pt", Inner), ("vals", ctypes.c_int32 * 2)]

o = Outer((3, 4), (5, 6))
assert (o.pt.x, o.pt.y, o.vals[0], o.vals[1]) == (3, 4, 5, 6)
o.pt.x = 99
o.vals[1] = 88
assert (o.pt.x, o.vals[1]) == (99, 88)

class IntBytes(ctypes.Union):
    _fields_ = [("u32", ctypes.c_uint32), ("b", ctypes.c_ubyte * 4)]

u = IntBytes()
u.u32 = 0x01020304
assert ctypes.sizeof(IntBytes) == 4
if sys.byteorder == "little":
    assert list(u.b) == [4, 3, 2, 1]
else:
    assert list(u.b) == [1, 2, 3, 4]

class AnonInner(ctypes.Structure):
    _fields_ = [("foo", ctypes.c_int32), ("bar", ctypes.c_int32)]

class WithAnon(ctypes.Structure):
    _anonymous_ = ("u",)
    _fields_ = [("tag", ctypes.c_int32), ("u", AnonInner)]

wa = WithAnon()
wa.tag = 1
wa.foo = 20
wa.bar = 30
assert (wa.tag, wa.foo, wa.bar, wa.u.foo, wa.u.bar) == (1, 20, 30, 20, 30)
print("structs and unions ok")

# 6. Deferred _fields_ (self-referential linked list node)
class Node(ctypes.Structure):
    pass

Node._fields_ = [("val", ctypes.c_int32), ("next", ctypes.POINTER(Node))]
n2 = Node(20, None)
n1 = Node(10, ctypes.pointer(n2))
assert n1.val == 10
assert n1.next.contents.val == 20
assert bool(n1.next.contents.next) is False

try:
    Node._fields_ = [("val", ctypes.c_int32)]
    assert False, "expected AttributeError for reassigning _fields_"
except AttributeError:
    pass
print("deferred fields ok")

# 7. Bitfields
class Flags(ctypes.Structure):
    _fields_ = [
        ("a", ctypes.c_uint32, 3),
        ("b", ctypes.c_uint32, 5),
        ("c", ctypes.c_int32, 4),
    ]

print("Flags:", ctypes.sizeof(Flags), Flags.a.offset, Flags.a.size, Flags.b.size, Flags.c.size)
fl = Flags(5, 19, -3)
assert (fl.a, fl.b, fl.c) == (5, 19, -3)
fl.a = 7
fl.c = -8
assert (fl.a, fl.b, fl.c) == (7, 19, -8)
print("bitfields ok")

# 8. Endianness (BigEndianStructure / LittleEndianStructure / BigEndianUnion / LittleEndianUnion)
class BEPacket(ctypes.BigEndianStructure):
    _pack_ = 1
    _fields_ = [("magic", ctypes.c_uint16), ("length", ctypes.c_uint32)]

class LEPacket(ctypes.LittleEndianStructure):
    _pack_ = 1
    _fields_ = [("magic", ctypes.c_uint16), ("length", ctypes.c_uint32)]

be = BEPacket(0x1234, 0xAABBCCDD)
le = LEPacket(0x1234, 0xAABBCCDD)
assert bytes(be) == b"\x12\x34\xaa\xbb\xcc\xdd"
assert bytes(le) == b"\x34\x12\xdd\xcc\xbb\xaa"
assert (be.magic, be.length) == (0x1234, 0xAABBCCDD)
assert (le.magic, le.length) == (0x1234, 0xAABBCCDD)

class BEUnion(ctypes.BigEndianUnion):
    _fields_ = [("u16", ctypes.c_uint16), ("raw", ctypes.c_ubyte * 2)]

bu = BEUnion()
bu.u16 = 0x1234
assert list(bu.raw) == [0x12, 0x34]
print("endian ok")

# 9. POINTER, pointer, byref, cast, create_string_buffer, create_unicode_buffer, string_at, wstring_at, memmove, memset
LP_int = ctypes.POINTER(ctypes.c_int32)
assert ctypes.POINTER(ctypes.c_int32) is LP_int
x = ctypes.c_int32(123)
px = ctypes.pointer(x)
assert bool(px) is True
assert px.contents.value == 123
assert px[0] == 123
px[0] = 456
assert x.value == 456

null_p = LP_int()
assert bool(null_p) is False
try:
    _ = null_p.contents
    assert False, "expected ValueError on NULL pointer"
except ValueError:
    pass

# cast
arr3 = (ctypes.c_int32 * 3)(100, 200, 300)
p_arr = ctypes.cast(arr3, ctypes.POINTER(ctypes.c_int32))
assert [p_arr[0], p_arr[1], p_arr[2]] == [100, 200, 300]
assert p_arr[0:3] == [100, 200, 300]
vp_arr = ctypes.cast(ctypes.byref(arr3, 4), ctypes.c_void_p)
assert vp_arr.value == ctypes.addressof(arr3) + 4

# create_string_buffer / create_unicode_buffer / string_at / wstring_at / memmove / memset
sb = ctypes.create_string_buffer(b"hello")
assert ctypes.sizeof(sb) == 6
assert sb.value == b"hello"
assert sb.raw == b"hello\x00"
assert ctypes.string_at(ctypes.addressof(sb)) == b"hello"
assert ctypes.string_at(sb, 3) == b"hel"

sb2 = ctypes.create_string_buffer(8)
ctypes.memset(sb2, ord("A"), 5)
assert sb2.value == b"AAAAA"
ctypes.memmove(ctypes.byref(sb2, 1), b"BC", 2)
assert sb2.value == b"ABCAA"

ub = ctypes.create_unicode_buffer("hello")
assert len(ub) == 6
assert ub.value == "hello"
assert ctypes.wstring_at(ub) == "hello"
assert ctypes.wstring_at(ctypes.addressof(ub), 4) == "hell"
print("memory helpers ok")

# 10. ctypes.wintypes
assert wt.MAX_PATH == 260
assert wt.RGB(0x11, 0x22, 0x33) == 0x332211
assert ctypes.sizeof(wt.BYTE) == 1
assert ctypes.sizeof(wt.WORD) == 2
assert ctypes.sizeof(wt.DWORD) == ctypes.sizeof(ctypes.c_ulong)
assert ctypes.sizeof(wt.HANDLE) == ctypes.sizeof(ctypes.c_void_p)
rect = wt.RECT(1, 2, 3, 4)
assert (rect.left, rect.top, rect.right, rect.bottom) == (1, 2, 3, 4)
pt = wt.POINT(10, 20)
assert (pt.x, pt.y) == (10, 20)
vb = wt.VARIANT_BOOL(True)
assert vb.value is True
assert ctypes.sizeof(wt.VARIANT_BOOL) == 2
assert issubclass(wt.PINT, ctypes._Pointer)
assert issubclass(wt.PRECT, ctypes._Pointer)
assert issubclass(wt.PWIN32_FIND_DATAA, ctypes._Pointer)
assert issubclass(wt.PWIN32_FIND_DATAW, ctypes._Pointer)
print("wintypes ok")

# 11. Exceptions and constants
assert issubclass(ctypes.ArgumentError, Exception)
assert ctypes.ArgumentError is _ctypes.ArgumentError
assert isinstance(ctypes.DEFAULT_MODE, int)
assert isinstance(ctypes.RTLD_GLOBAL, int)
assert isinstance(ctypes.RTLD_LOCAL, int)
print("ALL ctypes types parity tests passed!")
