# Issue #3339: EnumType.__members__, Enum.__format__, reversed(Enum),
# copy/deepcopy/pickle identity, member access through a member,
# __objclass__ and dir() alignment with CPython 3.12.3.
import copy
import pickle
from enum import Enum, Flag, IntEnum, IntFlag, StrEnum
from types import MappingProxyType


class E(Enum):
    A = 1
    B = 2
    C = 1  # alias of A


class I(IntEnum):
    X = 5
    Y = 6
    Z = 5  # alias of X


class S(StrEnum):
    P = "p"
    Q = "q"


class F(Flag):
    R = 1
    W = 2
    RW = 3


class IF(IntFlag):
    R = 1
    W = 2
    RW = 3


class M(int, Enum):
    Z = 3


class Custom(Enum):
    A = 1

    def __str__(self):
        return "custom"


class Mixed(str, Enum):
    U = "u"


class WithMethod(Enum):
    A = 1

    def describe(self):
        return self.name


ALL_CLASSES = [E, I, S, F, IF, M, Custom, Mixed, WithMethod]

# --- __members__ -----------------------------------------------------------
print(dict(E.__members__))
print(list(E.__members__))
print(type(E.__members__) is MappingProxyType, type(E.__members__).__name__)
print(len(E.__members__), len(E))
print(E.__members__["C"] is E.A, E.__members__["B"] is E.B)
print(list(I.__members__), list(S.__members__), list(F.__members__), list(IF.__members__))
print(dict(F.__members__))
for cls in ALL_CLASSES:
    print(cls.__name__, [(n, m is cls[n]) for n, m in cls.__members__.items()])
try:
    E.__members__["D"] = 3
except TypeError as exc:
    print("setitem", type(exc).__name__)
try:
    del E.__members__["A"]
except TypeError as exc:
    print("delitem", type(exc).__name__)
try:
    E.__members__ = {}
except AttributeError as exc:
    print("assign", type(exc).__name__)
print("D" in E.__members__, "A" in E.__members__)
# The proxy is live and read-only, not a copy that diverges.
proxy = E.__members__
print(proxy is E.__members__ or proxy == E.__members__)
print(list(Enum.__members__), list(Flag.__members__))
print(list(reversed(list(E.__members__))))
# __members__ is not reachable from a member instance.
print(hasattr(E.A, "__members__"))

# --- format() / f-strings ---------------------------------------------------
print(f"{E.A:>12}|")
print(f"{E.A:<12}|")
print(f"{E.A:^12}|")
print(f"{E.A:*^12}|")
print(f"{E.A}|", format(E.A), format(E.A, ""))
print(f"{E.C:>6}|")
print(f"{Custom.A:>10}|", format(Custom.A, ""))
print(f"{F.R:>12}|", f"{F.R | F.W:>16}|", f"{F.RW:<10}|")
print(f"{I.X:>4}|{I.X:03d}|{I.X}|{I.X:x}|{I.X:+d}")
print(f"{S.P:>4}|{S.P}|{S.P:^5}|{S.P:-<4}")
print(f"{IF.R:>4}|{IF.R | IF.W:03d}|{IF.RW}|{IF.RW:b}")
print(f"{M.Z:>6}|{M.Z}|{str(M.Z)}|{M.Z:<8}|")
print(f"{Mixed.U:>8}|{Mixed.U}|")
print(format(E.A, ">3"), format(E.A, "10"))
try:
    format(E.A, "d")
except ValueError as exc:
    print("bad spec", type(exc).__name__)
try:
    f"{E.A:03d}"
except ValueError as exc:
    print("bad spec 2", type(exc).__name__)
print("{:>8}|".format(E.B), "%s|%10s" % (E.A, E.B))
print(E.A.__format__(">9"), "|", I.X.__format__(">9"), "|", S.P.__format__(">9"))
print(E.__format__ is Enum.__format__, Enum.__format__.__name__)
# Subclasses defining their own __format__ keep it.
class Fmt(Enum):
    A = 1

    def __format__(self, spec):
        return "F<%s:%s>" % (self.name, spec)


print(f"{Fmt.A:xyz}")

# --- reversed() -----------------------------------------------------------------
print(list(reversed(E)))
print(list(reversed(I)), list(reversed(S)), list(reversed(F)), list(reversed(IF)))
print(list(reversed(M)), list(reversed(Custom)))
print([m.name for m in reversed(E)], [m.name for m in E])
print(type(E).__reversed__ is not None, hasattr(type(E), "__reversed__"))
it = reversed(E)
print(next(it), next(it))
try:
    next(it)
except StopIteration:
    print("exhausted")
print(list(reversed(Enum("Dyn", "a b c"))))


class Empty(Enum):
    pass


print(list(reversed(Empty)), list(Empty.__members__))

# --- copy / deepcopy ------------------------------------------------------------
members = [E.A, E.B, E.C, I.X, I.Z, S.P, F.R, F.RW, F.R | F.W, IF.W, IF.RW, M.Z, Custom.A, Mixed.U]
print([copy.copy(m) is m for m in members])
print([copy.deepcopy(m) is m for m in members])
print(copy.deepcopy([E.A, E.B], {})[0] is E.A)
print(copy.deepcopy({"k": (E.B, I.Y, S.Q)}) == {"k": (E.B, I.Y, S.Q)})
nested = {"a": [E.A, {"b": E.B}], "c": (I.X,)}
dup = copy.deepcopy(nested)
print(dup["a"][0] is E.A, dup["a"][1]["b"] is E.B, dup["c"][0] is I.X)
shallow = copy.copy(nested)
print(shallow["a"][0] is E.A)
print(E.A.__copy__() is E.A, E.A.__deepcopy__({}) is E.A)
pseudo = F(3) | F(0)
print(copy.copy(pseudo) is pseudo, copy.deepcopy(pseudo) is pseudo)
print(copy.deepcopy(E) is E, copy.copy(E) is E)

# --- __reduce_ex__ ----------------------------------------------------------------
for proto in range(6):
    print(proto, E.A.__reduce_ex__(proto), E.C.__reduce_ex__(proto))
    print(proto, I.X.__reduce_ex__(proto), S.P.__reduce_ex__(proto))
    print(proto, F.R.__reduce_ex__(proto), IF.RW.__reduce_ex__(proto), M.Z.__reduce_ex__(proto))

# --- pickle ----------------------------------------------------------------------------
for proto in range(pickle.HIGHEST_PROTOCOL + 1):
    print(proto, [pickle.loads(pickle.dumps(m, proto)) is m for m in members])
    print(
        proto,
        pickle.loads(pickle.dumps([E.A, E.B, E.A], proto))
        == [E.A, E.B, E.A],
    )
    out = pickle.loads(pickle.dumps({"x": E.A, "y": [I.Y, S.Q]}, proto))
    print(proto, out["x"] is E.A, out["y"][0] is I.Y, out["y"][1] is S.Q)
    # A repeated member is memoised but still resolves to the singleton.
    pair = pickle.loads(pickle.dumps((E.B, E.B), proto))
    print(proto, pair[0] is E.B, pair[1] is E.B)
print(pickle.loads(pickle.dumps(E.A)) is E.A)
print(pickle.loads(pickle.dumps(E)) is E)
try:
    pickle.loads(pickle.dumps(E.A, 2))._value_
    print("value", pickle.loads(pickle.dumps(E.A, 2))._value_)
except Exception as exc:
    print("unexpected", type(exc).__name__)

# --- member access through another member ----------------------------------------------
print(E.A.B, E.A.B.A, E.B.A.C)
print(E.A.B is E.B, E.A.C is E.A)
print(I.X.Y, S.P.Q, F.R.W, IF.R.W, M.Z.Z)
print(WithMethod.A.describe(), WithMethod.A.A.describe())
try:
    E.A.nope
except AttributeError as exc:
    print("AttributeError", exc)
try:
    E.nope
except AttributeError as exc:
    print("AttributeError", exc)
try:
    E.A.name.nope
except AttributeError as exc:
    print("AttributeError", exc)
print(E.A.name, E.A.value, E.A.B.name, E.A.B.value)

# --- __objclass__ ------------------------------------------------------------------------
for cls in ALL_CLASSES:
    print(cls.__name__, [(n, m.__objclass__ is cls) for n, m in cls.__members__.items()])
print(E.A.__objclass__, E.C.__objclass__, I.X.__objclass__, F.RW.__objclass__)
print(hasattr(E, "__objclass__"), "__objclass__" in dir(E.A))
print(hasattr(E.A.__objclass__, "A"))
print(E.A.__objclass__ is type(E.A))


class Sub(Enum):
    ONE = 1


print(Sub.ONE.__objclass__ is Sub, Sub.ONE.__objclass__ is E)

# --- dir() --------------------------------------------------------------------------------
print(dir(E))
print(dir(E.A))
print(dir(Empty))
print(dir(Empty))
print(dir(F))
print(dir(F.R))
print(dir(WithMethod))
print(dir(WithMethod.A))
print(dir(Custom), dir(Custom.A))
print("C" in dir(E), "A" in dir(E.A), "B" in dir(E.A), "name" in dir(E.A))
print("__members__" in dir(E), "__members__" in dir(E.A))
print("__objclass__" in dir(E), "__objclass__" in dir(Empty))
print(dir(E) == sorted(dir(E)), dir(E.A) == sorted(dir(E.A)))
print("describe" in dir(WithMethod.A), "describe" in dir(WithMethod))
for cls in (I, S, M, Mixed, IF):
    names = dir(cls)
    inst = dir(next(iter(cls)))
    print(
        cls.__name__,
        names == sorted(names),
        inst == sorted(inst),
        all(n in names for n in cls._member_names_),
        all(n in names for n in ("__members__", "__len__", "__iter__", "__contains__", "__getitem__", "__class__", "__module__")),
        all(n in inst for n in ("name", "value", "__class__", "__doc__", "__hash__", "__eq__", "__module__")),
        "__members__" in inst,
    )
print("bit_length" in dir(I), "bit_length" in dir(I.X), "upper" in dir(S.P), "upper" in dir(S))
