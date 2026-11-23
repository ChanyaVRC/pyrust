import array
import copy
import io
import math
import sys
import warnings
from array import ArrayType, array as Array, typecodes

warnings.simplefilter("ignore", DeprecationWarning)

# 1. Module surface and metadata
print("in sys.modules:", "array" in sys.modules)
print("dir public:", [n for n in dir(array) if not n.startswith("__")])
print("has __all__:", hasattr(array, "__all__"))
print("typecodes:", repr(typecodes))
print("ArrayType is array:", ArrayType is Array)
print("module:", Array.__module__)

# 2. All typecodes, itemsizes, and empty reprs
for tc in typecodes:
    a = Array(tc)
    print("tc", tc, "itemsize:", a.itemsize, "len:", len(a), "bool:", bool(a), "repr:", repr(a))

# 3. Numeric construction, bounds, precision, and repr
a_b = Array("b", [-128, 0, 127])
a_B = Array("B", [0, 128, 255])
a_h = Array("h", [-32768, 0, 32767])
a_H = Array("H", [0, 65535])
a_i = Array("i", [-2147483648, 0, 2147483647])
a_I = Array("I", [0, 4294967295])
a_l = Array("l", [-123456, 123456])
a_L = Array("L", [0, 123456])
a_q = Array("q", [-9223372036854775808, 0, 9223372036854775807])
a_Q = Array("Q", [0, 18446744073709551615])
a_f = Array("f", [1.5, -2.0, 1.1, 1e39])
a_d = Array("d", [1.5, -2.25, float("inf"), float("-inf")])

for arr in [a_b, a_B, a_h, a_H, a_i, a_I, a_l, a_L, a_q, a_Q, a_f, a_d]:
    print("arr:", repr(arr), "tolist:", arr.tolist(), "roundtrip:", Array(arr.typecode, arr.tobytes()) == arr)

# Bool and __index__ and __float__ coercions
class MyIndex:
    def __index__(self):
        return 7


class MyFloat:
    def __float__(self):
        return 3.5


print("bool/index int:", Array("i", [True, False, MyIndex()]))
print("float/index/dunder:", Array("f", [1, MyIndex(), MyFloat()]))

# 4. Unicode ('u') array
u = Array("u", "hello")
print("u repr:", repr(u), "len:", len(u), "tounicode:", repr(u.tounicode()), "tolist:", u.tolist())
u.fromunicode("!")
print("u after fromunicode:", repr(u), "u[0]:", repr(u[0]), "u[-1]:", repr(u[-1]))
u_rt = Array("u", u.tobytes())
print("u bytes roundtrip:", u_rt == u, repr(u_rt))
u_list = Array("u", ["a", "b"])
print("u list init:", repr(u_list))

# 5. Initializer variants (bytes, bytearray, same-kind array, diff-kind array, generator)
raw_i = Array("i", [10, 20, 30]).tobytes()
print("init bytes:", Array("i", raw_i))
print("init bytearray:", Array("i", bytearray(raw_i)))
print("init same array:", Array("i", Array("i", [1, 2])))
print("init diff array:", Array("i", Array("b", [1, 2])))
print("init generator:", Array("h", (x * 2 for x in range(4))))

# 6. Mutation methods: append, extend, insert, pop, remove, reverse, index, count, byteswap
m = Array("i", [10, 20])
m.append(30)
m.extend([40, 50])
m.extend(Array("i", [60]))
print("after append/extend:", m)

ms = Array("i", [1, 2])
ms.extend(ms)
print("self extend:", ms)

m.insert(0, 5)
m.insert(-1, 55)
m.insert(100, 70)
m.insert(-100, 1)
print("after insert:", m)

print("pop default:", m.pop(), "pop(0):", m.pop(0), "pop(-2):", m.pop(-2), "rem:", m)
m.remove(20)
print("after remove(20):", m)
m.reverse()
print("after reverse:", m)

idx_a = Array("i", [10, 20, 30, 20, 40])
print("index:", idx_a.index(20), idx_a.index(20, 2), idx_a.index(20, -3), idx_a.index(20, -4, -1))
print("count:", idx_a.count(20), idx_a.count(20.0), idx_a.count(99))
print("contains:", 20 in idx_a, 20.0 in idx_a, 99 in idx_a)

bs = Array("H", [0x0102, 0x0304])
bs.byteswap()
print("byteswap H:", bs.tolist())
bs.byteswap()
print("byteswap H twice:", bs.tolist())
bs_b = Array("b", [1, -2])
bs_b.byteswap()
print("byteswap b:", bs_b.tolist())
bs_f = Array("f", [1.5, -2.5])
bs_f.byteswap()
bs_f.byteswap()
print("byteswap f twice:", bs_f.tolist())

# 7. Serialization: tobytes, frombytes, tolist, fromlist (atomic), tofile, fromfile, buffer_info
s = Array("i", [1, 2])
s.frombytes(Array("i", [3, 4]).tobytes())
s.frombytes(bytearray(Array("i", [5]).tobytes()))
print("frombytes:", s)

s.fromlist([6, 7])
print("fromlist ok:", s)
try:
    s.fromlist([8, "bad"])
except TypeError:
    print("fromlist atomic unchanged:", s)

ep = Array("i", [1])
try:
    ep.extend([2, "bad"])
except TypeError:
    print("extend partial:", ep)

bio = io.BytesIO()
s.tofile(bio)
bio.seek(0)
s2 = Array("i")
s2.fromfile(bio, len(s))
print("tofile/fromfile:", s2 == s, s2)

bio_short = io.BytesIO(Array("i", [10]).tobytes())
s_short = Array("i", [99])
try:
    s_short.fromfile(bio_short, 2)
except EOFError:
    print("fromfile partial eof:", s_short)

bi = s.buffer_info()
print("buffer_info:", len(bi), isinstance(bi[0], int), bi[1] == len(s))

# 8. Iteration, reversed, indexing, slicing, slice assignment, deletion
it_a = Array("i", [10, 20, 30, 40, 50])
print("iter:", list(it_a), "reversed:", list(reversed(it_a)))
print("getitem:", it_a[0], it_a[-1], it_a[1:4], it_a[::2], it_a[::-1])

sl = Array("i", [1, 2, 3, 4, 5])
sl[1] = 20
sl[-1] = 50
print("setitem:", sl)
sl[1:3] = Array("i", [200, 300, 400])
print("setitem slice grow:", sl)
sl[::2] = Array("i", [9, 8, 7])
print("setitem ext slice:", sl)
sl[:] = sl
print("setitem self slice:", sl)
del sl[0]
del sl[1:3]
print("delitem:", sl)
sl2 = Array("i", [0, 1, 2, 3, 4, 5])
del sl2[::2]
print("del ext slice:", sl2)

# 9. Operators (+, +=, *, *=)
o1 = Array("i", [1, 2])
o2 = Array("i", [3, 4])
print("add:", o1 + o2)
o1 += o2
print("iadd:", o1)
print("mul:", Array("i", [1, 2]) * 3, 3 * Array("i", [1, 2]), Array("i", [1, 2]) * 0, Array("i", [1, 2]) * -2)
print("mul __index__:", Array("i", [1, 2]) * MyIndex(), MyIndex() * Array("i", [1, 2]))
om = Array("i", [7, 8])
om *= 2
print("imul:", om)
om *= 0
print("imul 0:", om)

# 10. Comparisons (cross-typecode, list, NaN, ordering)
c1 = Array("i", [1, 2])
c2 = Array("q", [1, 2])
c3 = Array("d", [1.0, 2.0])
print("eq cross:", c1 == c2, c1 == c3, c1 == [1, 2], c1 != [1, 2], Array("u", "a") == Array("i", [97]))
print("ord:", c1 < Array("q", [1, 3]), c1 <= c2, c1 > Array("i", [1]), c1 >= c3, Array("i", [1]) < c1)
cn = Array("d", [float("nan")])
print("nan cmp:", cn == cn, cn != cn, cn < cn, cn <= cn, cn > cn, cn >= cn)

# 11. Copy, deepcopy, and subclassing
cp = Array("i", [1, 2, 3])
cp1 = copy.copy(cp)
cp2 = copy.deepcopy(cp)
print("copy:", cp1 == cp, cp1 is not cp, cp2 == cp, cp2 is not cp)


class SubArr(Array):
    pass


sa = SubArr("i", [1, 2])
print(
    "subclass:",
    repr(sa),
    type(sa[:1]) is Array,
    type(sa + sa) is Array,
    type(sa * 2) is Array,
    type(copy.copy(sa)) is Array,
)

# 12. Read-only attributes and unhashability
try:
    cp.typecode = "d"
    print("UNEXPECTED typecode set")
except AttributeError:
    print("typecode readonly: AttributeError")

try:
    cp.itemsize = 99
    print("UNEXPECTED itemsize set")
except AttributeError:
    print("itemsize readonly: AttributeError")

try:
    hash(cp)
    print("UNEXPECTED hash")
except TypeError:
    print("unhashable: TypeError")

# 13. Error cases
def check_exc(label, exc_type, fn):
    try:
        fn()
        print("UNEXPECTED OK:", label)
    except exc_type as e:
        print("OK EXC:", label, type(e).__name__)


check_exc("bad tc z", ValueError, lambda: Array("z"))
check_exc("bad tc empty", TypeError, lambda: Array(""))
check_exc("bad tc multi", TypeError, lambda: Array("ii"))
check_exc("bad tc int", TypeError, lambda: Array(1))
check_exc("b overflow", OverflowError, lambda: Array("b", [128]))
check_exc("b underflow", OverflowError, lambda: Array("b", [-129]))
check_exc("B neg", OverflowError, lambda: Array("B", [-1]))
check_exc("B overflow", OverflowError, lambda: Array("B", [256]))
check_exc("q overflow", OverflowError, lambda: Array("q", [1 << 100]))
check_exc("Q neg", OverflowError, lambda: Array("Q", [-1]))
check_exc("i float", TypeError, lambda: Array("i", [1.5]))
check_exc("i str elem", TypeError, lambda: Array("i", ["1"]))
check_exc("i str init", TypeError, lambda: Array("i", "1234"))
check_exc("u bad len", TypeError, lambda: Array("u", ["ab"]))
check_exc("u bad int", TypeError, lambda: Array("u", [65]))
check_exc("bytes bad len", ValueError, lambda: Array("i", b"\x01\x02"))
check_exc("extend diff array", TypeError, lambda: Array("i", [1]).extend(Array("b", [2])))
check_exc("fromlist non-list", TypeError, lambda: Array("i").fromlist((1, 2)))
check_exc("tounicode non-u", ValueError, lambda: Array("i").tounicode())
check_exc("fromunicode non-u", ValueError, lambda: Array("i").fromunicode("a"))
check_exc("fromunicode non-str", TypeError, lambda: Array("u").fromunicode(b"a"))
check_exc("add diff array", TypeError, lambda: Array("i", [1]) + Array("b", [2]))
check_exc("add list", TypeError, lambda: Array("i", [1]) + [2])
check_exc("iadd diff array", TypeError, lambda: Array("i", [1]).__iadd__(Array("b", [2])))
check_exc("setitem slice list", TypeError, lambda: Array("i", [1, 2]).__setitem__(slice(None), [3, 4]))
check_exc("setitem slice diff array", TypeError, lambda: Array("i", [1, 2]).__setitem__(slice(None), Array("b", [3, 4])))
check_exc("setitem ext slice bad len", ValueError, lambda: Array("i", [1, 2, 3, 4]).__setitem__(slice(0, 4, 2), Array("i", [9])))
check_exc("pop empty", IndexError, lambda: Array("i").pop())
check_exc("pop oob", IndexError, lambda: Array("i", [1]).pop(5))
check_exc("remove missing", ValueError, lambda: Array("i", [1, 2]).remove(9))
check_exc("index missing", ValueError, lambda: Array("i", [1, 2]).index(9))
check_exc("lt list", TypeError, lambda: Array("i", [1]) < [1])
check_exc("u lt i", TypeError, lambda: Array("u", "a") < Array("i", [97]))
