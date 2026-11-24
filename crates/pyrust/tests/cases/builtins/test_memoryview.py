import array
import base64
import binascii
import collections.abc
import hashlib
import inspect
import io
import struct
import zlib


def check_exc(label, exc_type, fn):
    try:
        fn()
    except exc_type as exc:
        print(label, "OK:", type(exc).__name__)
    else:
        print(label, "FAILED: no exception raised")


# 1. Type identity, builtins registration, ABCs, non-subclassable
mv = memoryview(b"hello")
print("type name:", type(mv).__name__, type(mv) is memoryview)
print("isinstance memoryview:", isinstance(mv, memoryview))
print("isinstance Buffer:", isinstance(mv, collections.abc.Buffer))
print("isinstance Sequence:", isinstance(mv, collections.abc.Sequence))
print("isinstance Reversible:", isinstance(mv, collections.abc.Reversible))
print("isinstance Hashable:", isinstance(mv, collections.abc.Hashable))
print("isinstance MutableSequence:", isinstance(mv, collections.abc.MutableSequence))
print("issubclass Buffer:", issubclass(memoryview, collections.abc.Buffer))
print("issubclass Sequence:", issubclass(memoryview, collections.abc.Sequence))
print(
    "builtins Buffer checks:",
    isinstance(b"x", collections.abc.Buffer),
    isinstance(bytearray(b"x"), collections.abc.Buffer),
    isinstance(memoryview(b"x"), collections.abc.Buffer),
    isinstance(array.array("i", [1]), collections.abc.Buffer),
    isinstance("x", collections.abc.Buffer),
    isinstance(123, collections.abc.Buffer),
)
print(
    "issubclass Buffer builtins:",
    issubclass(bytes, collections.abc.Buffer),
    issubclass(bytearray, collections.abc.Buffer),
    issubclass(memoryview, collections.abc.Buffer),
    issubclass(array.array, collections.abc.Buffer),
)
check_exc("cannot subclass memoryview", TypeError, lambda: type("SubMV", (memoryview,), {}))

# 2. Attributes on bytes memoryview
b = b"hello"
mv_b = memoryview(b)
print(
    "bytes attrs:",
    mv_b.obj is b,
    mv_b.nbytes,
    mv_b.readonly,
    mv_b.format,
    mv_b.itemsize,
    mv_b.ndim,
    mv_b.shape,
    mv_b.strides,
    mv_b.suboffsets,
    mv_b.c_contiguous,
    mv_b.f_contiguous,
    mv_b.contiguous,
)
print("len/bool:", len(mv_b), bool(mv_b), len(memoryview(b"")), bool(memoryview(b"")))
print("repr active:", repr(mv_b).startswith("<memory at 0x") and repr(mv_b).endswith(">"))
check_exc("readonly attr write", AttributeError, lambda: setattr(mv_b, "readonly", False))

# 3. bytearray zero-copy live view (both directions) + export count
ba = bytearray(b"abcdef")
mv_ba = memoryview(ba)
print("ba mv readonly:", mv_ba.readonly, mv_ba.obj is ba)
mv_ba[0] = ord("A")
print("mv->ba single write:", ba)
ba[1] = ord("B")
print("ba->mv single write:", mv_ba[1], mv_ba.tobytes())
mv_ba[2:5] = b"CDE"
print("mv->ba slice write:", ba)
mv_ba[::2] = b"123"
print("mv->ba stepped slice write:", ba)

# Export count prevents resize until released
check_exc("ba append while exported", BufferError, lambda: ba.append(1))
check_exc("ba clear while exported", BufferError, lambda: ba.clear())
check_exc("ba pop while exported", BufferError, lambda: ba.pop())

# Slicing returns a zero-copy sub-memoryview sharing ManagedBuffer
sub = mv_ba[1:5]
print("sub type/bytes/strides:", type(sub) is memoryview, sub.tobytes(), sub.shape, sub.strides)
sub[0] = ord("X")
print("sub->ba write:", ba, mv_ba.tobytes())
sub_step = mv_ba[::-2]
print(
    "stepped sub:",
    sub_step.tobytes(),
    sub_step.tolist(),
    sub_step.shape,
    sub_step.strides,
    sub_step.c_contiguous,
    sub_step.f_contiguous,
    sub_step.contiguous,
)

mv_ba.release()
# sub and sub_step are still alive so ba is still locked
check_exc("ba append with sub alive", BufferError, lambda: ba.append(1))
sub.release()
sub_step.release()
ba.append(ord("!"))
print("ba append after all released:", ba)

# 4. Readonly errors and delete error
ro = memoryview(b"abcd")
check_exc("ro setitem", TypeError, lambda: ro.__setitem__(0, 65) if hasattr(ro, "__setitem__") else exec("ro[0] = 65", {"ro": ro}))
def assign_ro_idx():
    ro[0] = 65
def assign_ro_slice():
    ro[0:2] = b"AB"
def del_mv_idx():
    m = memoryview(bytearray(b"ab"))
    del m[0]
check_exc("ro idx assign", TypeError, assign_ro_idx)
check_exc("ro slice assign", TypeError, assign_ro_slice)
check_exc("del mv", TypeError, del_mv_idx)

# 5. array.array zero-copy live view
arr = array.array("i", [10, 20, 30, 40])
mv_arr = memoryview(arr)
print(
    "array mv:",
    mv_arr.format,
    mv_arr.itemsize,
    mv_arr.nbytes,
    mv_arr.ndim,
    mv_arr.shape,
    mv_arr.strides,
    mv_arr.readonly,
    mv_arr.tolist(),
    len(mv_arr),
)
mv_arr[1] = -99
print("mv->array write:", list(arr))
arr[2] = 777
print("array->mv write:", mv_arr[2], mv_arr.tolist())
print("mv == array:", mv_arr == arr, arr == mv_arr)
check_exc("array append while exported", BufferError, lambda: arr.append(50))
mv_arr.release()
arr.append(50)
print("array append after release:", list(arr))

# 6. memoryview from another memoryview
ba2 = bytearray(b"012345")
m1 = memoryview(ba2)
m2 = memoryview(m1)
m2[0] = ord("Z")
print("mv(mv) live:", ba2, m1.tobytes())
m1.release()
print("m2 survives m1.release:", m2.tobytes())
m2.release()

# 7. io.BytesIO.getbuffer(), BytesIO(mv), BytesIO.write(mv), BytesIO.readinto(mv)
bio = io.BytesIO(memoryview(b"hello world"))
print("BytesIO(mv):", bio.getvalue())
gb = bio.getbuffer()
print("getbuffer:", type(gb) is memoryview, gb.readonly, gb.format, gb.tobytes())
gb[0] = ord("H")
gb[6:11] = b"WORLD"
print("getbuffer live mutation:", bio.getvalue())
check_exc("BytesIO write while exported", BufferError, lambda: bio.write(b"!"))
check_exc("BytesIO truncate while exported", BufferError, lambda: bio.truncate(2))
check_exc("BytesIO close while exported", BufferError, lambda: bio.close())
gb.release()
bio.seek(0, 2)
bio.write(memoryview(b"!!!"))
print("BytesIO write(mv):", bio.getvalue())
bio.seek(0)
dst_ba = bytearray(5)
dst_mv = memoryview(dst_ba)
n_read = bio.readinto(dst_mv)
print("BytesIO readinto(mv):", n_read, dst_ba)
dst_mv.release()
bio.close()

# 8. Methods: tobytes, tolist, hex, toreadonly, cast (1D, 2D, 0D, formats)
raw = bytearray(b"\xde\xad\xbe\xef")
m_raw = memoryview(raw)
print("hex():", m_raw.hex())
print("hex(':'):", m_raw.hex(":"))
print("hex(':', 2):", m_raw.hex(":", 2))
print("hex(':', -2):", m_raw.hex(":", -2))
print("hex kw:", m_raw.hex(sep="-", bytes_per_sep=2))

m_ro = m_raw.toreadonly()
print("toreadonly:", m_ro.readonly, m_ro.tobytes() == bytes(raw))
def assign_m_ro():
    m_ro[0] = 0
check_exc("toreadonly write", TypeError, assign_m_ro)
m_raw.release()
m_ro.release()

# cast formats
ba_num = bytearray(8)
mv_i = memoryview(ba_num).cast("i")
mv_i[0] = -42
mv_i[1] = 123456
print("cast i:", mv_i.format, mv_i.itemsize, mv_i.shape, mv_i.strides, mv_i.tolist())
check_exc("cast non-byte to non-byte", TypeError, lambda: mv_i.cast("H"))
mv_h = mv_i.cast("B").cast("H")
print("cast i->B->H:", mv_h.format, mv_h.itemsize, mv_h.shape, len(mv_h))
mv_f = memoryview(bytearray(8)).cast("f")
mv_f[0] = 1.5
mv_f[1] = -2.25
print("cast f:", mv_f.tolist())
mv_d = memoryview(bytearray(8)).cast("d")
mv_d[0] = 3.125
print("cast d:", mv_d.tolist())
mv_q = memoryview(bytearray(8)).cast("q")
mv_q[0] = -1000000000000
print("cast q:", mv_q.tolist())
mv_c = memoryview(b"xy").cast("c")
print("cast c:", mv_c[0], mv_c.tolist())
mv_bool = memoryview(b"\x00\x01\x05").cast("?")
print("cast ?:", mv_bool.tolist())

# 2D cast & order C/F/A
ba_2d = bytearray(range(6))
mv_2d = memoryview(ba_2d).cast("B", shape=(2, 3))
print(
    "2d attrs:",
    mv_2d.ndim,
    mv_2d.shape,
    mv_2d.strides,
    mv_2d.nbytes,
    len(mv_2d),
    mv_2d.c_contiguous,
    mv_2d.f_contiguous,
    mv_2d.contiguous,
)
print("2d tolist:", mv_2d.tolist())
print("2d tobytes C/F/A:", mv_2d.tobytes("C"), mv_2d.tobytes("F"), mv_2d.tobytes("A"))
print("2d[1, 2]:", mv_2d[1, 2])
mv_2d[1, 2] = 99
print("2d after assign:", mv_2d.tolist(), list(ba_2d))
check_exc("2d single idx", NotImplementedError, lambda: mv_2d[0])
check_exc("tobytes bad order", ValueError, lambda: mv_2d.tobytes("Z"))
mv_2d.release()

# 0D cast
mv_0d = memoryview(b"Z").cast("B", ())
print("0d attrs:", mv_0d.ndim, mv_0d.shape, mv_0d.strides, mv_0d.nbytes, mv_0d[()], mv_0d.tolist())
check_exc("0d len", TypeError, lambda: len(mv_0d))
check_exc("0d bool", TypeError, lambda: bool(mv_0d))

# cast errors
check_exc("cast misaligned", TypeError, lambda: memoryview(b"abc").cast("h"))
check_exc("cast non-contig", TypeError, lambda: memoryview(b"abcd")[::2].cast("h"))
check_exc("cast bad format", ValueError, lambda: memoryview(b"abcd").cast("<i"))
check_exc("cast bad shape product", TypeError, lambda: memoryview(b"abcd").cast("B", (2, 3)))
check_exc("cast neg shape", ValueError, lambda: memoryview(b"abcd").cast("B", (-1, 4)))

# 9. Equality, hashing, dict/set keys, iteration, membership, reversed
print(
    "eq checks:",
    memoryview(b"abc") == b"abc",
    b"abc" == memoryview(b"abc"),
    memoryview(b"abc") == bytearray(b"abc"),
    bytearray(b"abc") == memoryview(b"abc"),
    memoryview(b"abc") == memoryview(b"abc"),
    memoryview(b"abc") != memoryview(b"abd"),
    memoryview(b"\x01\x02").cast("B") == memoryview(b"\x01\x02").cast("b"),
    memoryview(b"\xff").cast("B") == memoryview(b"\xff").cast("b"),
    memoryview(b"abcd").cast("B", (2, 2)) == b"abcd",
    memoryview(b"abcd").cast("B", (2, 2)) == memoryview(b"abcd").cast("B", (2, 2)),
    memoryview(b"abc") == [97, 98, 99],
)

mv_h1 = memoryview(b"hashme")
print("hash match bytes:", hash(mv_h1) == hash(b"hashme"))
print("hash stepped match tobytes:", hash(mv_h1[::2]) == hash(mv_h1[::2].tobytes()))
print("hash 2d match bytes:", hash(memoryview(b"abcdef").cast("B", (2, 3))) == hash(b"abcdef"))
print("dict cross lookup:", {b"abc": 1}[memoryview(b"abc")], {memoryview(b"abc"): 2}[b"abc"])
print("set dedup:", len({b"abc", memoryview(b"abc")}))
check_exc("hash writable", ValueError, lambda: hash(memoryview(bytearray(b"abc"))))
check_exc("hash ba toreadonly", TypeError, lambda: hash(memoryview(bytearray(b"abc")).toreadonly()))
check_exc("hash int format", ValueError, lambda: hash(memoryview(b"1234").cast("i")))

print("iter/list:", list(memoryview(b"abc")))
print("reversed:", list(reversed(memoryview(b"abc"))))
print("in:", 97 in memoryview(b"abc"), 999 in memoryview(b"abc"), b"a" in memoryview(b"abc"))

# 10. Context manager & release idempotence
ba_cm = bytearray(b"ctx")
with memoryview(ba_cm) as cm:
    cm[0] = ord("C")
    print("inside with:", cm.tobytes())
print("after with:", ba_cm)
print("released repr:", repr(cm).startswith("<released memory at 0x") and repr(cm).endswith(">"))
print("released eq:", cm == cm, cm == memoryview(b"Ctx"), cm == b"Ctx")
print("idempotent release:", cm.release())
check_exc("released len", ValueError, lambda: len(cm))
check_exc("released bool", ValueError, lambda: bool(cm))
check_exc("released format", ValueError, lambda: cm.format)
check_exc("released tobytes", ValueError, lambda: cm.tobytes())
check_exc("released hash", ValueError, lambda: hash(cm))
check_exc("released getitem", ValueError, lambda: cm[0])
check_exc("released iter", ValueError, lambda: list(cm))
check_exc("released in", ValueError, lambda: 67 in cm)

# 11. PEP 688 custom __buffer__ and __release_buffer__ + inspect.BufferFlags
events = []


class CustomBuffer:
    def __init__(self, data):
        self._buf = bytearray(data)

    def __buffer__(self, flags):
        events.append(("buffer", flags))
        return memoryview(self._buf)

    def __release_buffer__(self, view):
        events.append(("release", view.tobytes()))
        view.release()


cb = CustomBuffer(b"pep688")
print("isinstance CustomBuffer Buffer:", isinstance(cb, collections.abc.Buffer))
print("issubclass CustomBuffer Buffer:", issubclass(CustomBuffer, collections.abc.Buffer))
with memoryview(cb) as cmv:
    cmv[0] = ord("P")
    print("custom mv:", cmv.tobytes(), cmv.readonly)
print("custom events:", events)
print("custom mutated:", cb._buf)

# Slice keeps __release_buffer__ deferred until last view is released
events.clear()
mv_outer = memoryview(cb)
mv_slice = mv_outer[1:4]
mv_outer.release()
print("events while slice alive:", len(events))
mv_slice.release()
print("events after slice released:", len(events))

class BadBuffer:
    def __buffer__(self, flags):
        return b"not a memoryview"

check_exc("bad __buffer__ return", TypeError, lambda: memoryview(BadBuffer()))
check_exc("non-buffer int", TypeError, lambda: memoryview(42))
check_exc("non-buffer str", TypeError, lambda: memoryview("hi"))

# inspect.BufferFlags check
print(
    "BufferFlags:",
    int(inspect.BufferFlags.SIMPLE),
    int(inspect.BufferFlags.WRITABLE),
    int(inspect.BufferFlags.FULL_RO),
)

# 12. Stdlib & builtin consumers with memoryview & PEP 688
m_src = memoryview(b"\x01\x02\x03\x04")
print("bytes(mv):", bytes(m_src))
print("bytearray(mv):", bytearray(m_src))
print("bytes(CustomBuffer):", bytes(CustomBuffer(b"xyz")))
print("bytearray(CustomBuffer):", bytearray(CustomBuffer(b"xyz")))
print("int(mv):", int(memoryview(b"42")))
print("float(mv):", float(memoryview(b"3.5")))
print("int.from_bytes(mv):", int.from_bytes(memoryview(b"\x01\x00"), "big"))

print("struct.unpack(mv):", struct.unpack("<HH", m_src))
pack_ba = bytearray(8)
pack_mv = memoryview(pack_ba)[2:6]
struct.pack_into("<I", pack_mv, 0, 0x12345678)
print("struct.pack_into(sliced mv):", pack_ba.hex())
pack_mv.release()

cb_pack = CustomBuffer(b"\x00\x00\x00\x00")
struct.pack_into("<I", cb_pack, 0, 0xDEADBEEF)
print("struct.pack_into(CustomBuffer):", cb_pack._buf.hex())
print("struct.unpack(CustomBuffer):", struct.unpack("<I", cb_pack))

# binascii, base64, hashlib, zlib
m_hello = memoryview(b"hello world")
print("binascii.hexlify(mv):", binascii.hexlify(m_hello))
print("base64.b64encode(mv):", base64.b64encode(m_hello))
print("base64.b64decode(mv):", base64.b64decode(memoryview(b"aGVsbG8=")))
print("hashlib.sha256(mv):", hashlib.sha256(m_hello).hexdigest())
comp = zlib.compress(m_hello)
print("zlib roundtrip(mv):", zlib.decompress(memoryview(comp)))
print("concat bytes+mv:", b"ab" + memoryview(b"cd"))
print("concat ba+mv:", bytearray(b"ab") + memoryview(b"cd"))
ba_iadd = bytearray(b"ab")
ba_iadd += memoryview(b"cd")
print("iadd ba+=mv:", ba_iadd)
