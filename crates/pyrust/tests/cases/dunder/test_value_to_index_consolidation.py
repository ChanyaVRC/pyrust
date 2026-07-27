# Parity fixture for issue #2022: the ~40 open-coded integer/__index__
# coercions were consolidated into a single `value_to_index` helper.  After the
# refactor EVERY interpreter-reachable index context honors CPython 3.12's
# index protocol uniformly: int, bool, an int-subclass, and any object with
# __index__ are accepted; float and __int__-only objects are rejected with the
# context-specific TypeError; __index__ returning a non-int raises TypeError.
#
# Contexts covered: subscripts (list/str/tuple/bytes), slice bounds, sequence
# repetition (`seq * n` and `list.__mul__`), range, chr/hex/oct/bin,
# list.insert/list.pop/list.index, int.to_bytes length, itertools
# islice/repeat/tee/batched/combinations counts, math.factorial/comb/perm,
# sys.setrecursionlimit, and Unicode{Decode,Encode,Translate}Error start/end.


class Idx:
    def __init__(self, v):
        self.v = v

    def __index__(self):
        return self.v


class MyInt(int):
    pass


class IntOnly:
    def __int__(self):
        return 3


class BadIndex:
    def __index__(self):
        return "not an int"


class RaisingIndex:
    def __index__(self):
        raise ValueError("index boom")


def show(label, fn):
    try:
        print(label, "=", fn())
    except Exception as e:
        print(label, "!", type(e).__name__, e)


# ---- subscripts ----
show("list[Idx]", lambda: [10, 20, 30, 40][Idx(3)])
show("list[MyInt]", lambda: [10, 20, 30, 40][MyInt(2)])
show("str[Idx]", lambda: "abcde"[Idx(1)])
show("tuple[Idx]", lambda: (1, 2, 3, 4)[Idx(2)])
show("bytes[Idx]", lambda: b"abcde"[Idx(4)])
_f = 1.0
show("list[float]", lambda: [1, 2, 3][_f])
show("str[IntOnly]", lambda: "abc"[IntOnly()])
show("bytes[BadIndex]", lambda: b"abc"[BadIndex()])

# ---- slice bounds ----
show("str[Idx:Idx]", lambda: "abcdefgh"[Idx(2):Idx(5)])
show("list[:Idx]", lambda: [0, 1, 2, 3, 4][: Idx(3)])
show("list[Idx::Idx]", lambda: [0, 1, 2, 3, 4, 5][Idx(1):: Idx(2)])

# ---- sequence repetition ----
show("[0]*Idx", lambda: [0] * Idx(3))
show("str*Idx", lambda: "ab" * Idx(2))
show("Idx*tuple", lambda: Idx(2) * (1, 2))
show("list.__mul__(Idx)", lambda: [1, 2].__mul__(Idx(2)))
show("list*float", lambda: [0] * 1.0)
show("list.__mul__(IntOnly)", lambda: [1].__mul__(IntOnly()))

# ---- range ----
show("range(Idx)", lambda: list(range(Idx(4))))
show("range(Idx,Idx)", lambda: list(range(Idx(1), Idx(5))))
show("range(float)", lambda: range(1.0))
show("range(IntOnly)", lambda: range(IntOnly()))

# ---- chr / hex / oct / bin ----
show("chr(Idx)", lambda: chr(Idx(66)))
show("chr(MyInt)", lambda: chr(MyInt(67)))
show("hex(Idx)", lambda: hex(Idx(255)))
show("oct(Idx)", lambda: oct(Idx(8)))
show("bin(Idx)", lambda: bin(Idx(5)))
show("hex(IntOnly)", lambda: hex(IntOnly()))
show("chr(float)", lambda: chr(65.0))
show("bin(BadIndex)", lambda: bin(BadIndex()))
show("hex(bigidx)", lambda: hex(Idx(10**30)))

# ---- list.insert / pop / index ----
def _insert():
    a = [1, 2, 3]
    a.insert(Idx(1), 99)
    return a


def _pop():
    a = [10, 20, 30]
    return (a.pop(Idx(1)), a)


show("list.insert(Idx)", _insert)
show("list.pop(Idx)", _pop)
show("list.index(start=Idx)", lambda: [5, 6, 7, 6].index(6, Idx(2)))
show("list.insert(float)", lambda: [1, 2].insert(1.0, 9))
show("list.pop(str)", lambda: [1, 2].pop("x"))

# ---- int.to_bytes length ----
show("to_bytes(Idx)", lambda: (255).to_bytes(Idx(2), "big"))
show("to_bytes(MyInt)", lambda: (1).to_bytes(MyInt(3), "big"))
show("to_bytes(float)", lambda: (1).to_bytes(2.0, "big"))
show("to_bytes(IntOnly)", lambda: (1).to_bytes(IntOnly(), "big"))
show("to_bytes(BadIndex)", lambda: (1).to_bytes(BadIndex(), "big"))
show("to_bytes(RaisingIndex)", lambda: (1).to_bytes(RaisingIndex(), "big"))

# ---- itertools counts ----
import itertools

show("islice(Idx)", lambda: list(itertools.islice([1, 2, 3, 4, 5], Idx(3))))
show("islice(Idx,Idx)", lambda: list(itertools.islice([1, 2, 3, 4, 5], Idx(1), Idx(4))))
show("repeat(Idx)", lambda: list(itertools.repeat("a", Idx(3))))
show("tee(Idx)", lambda: [list(t) for t in itertools.tee([1, 2], Idx(2))])
show("batched(Idx)", lambda: list(itertools.batched("abcdef", Idx(2))))
show("combinations(Idx)", lambda: list(itertools.combinations("abcd", Idx(2))))
show("product(repeat=Idx)", lambda: list(itertools.product("ab", repeat=Idx(2))))
show("islice(float)", lambda: list(itertools.islice([1, 2, 3], 1.5)))
show("repeat(IntOnly)", lambda: list(itertools.repeat("a", IntOnly())))
show("batched(0)", lambda: list(itertools.batched("ab", 0)))
show("combinations(neg)", lambda: list(itertools.combinations("ab", -1)))
show("tee(float)", lambda: list(itertools.tee([1], 1.5)))

# ---- math ----
import math

show("factorial(Idx)", lambda: math.factorial(Idx(5)))
show("comb(Idx,Idx)", lambda: math.comb(Idx(5), Idx(2)))
show("perm(Idx,Idx)", lambda: math.perm(Idx(5), Idx(2)))
show("isqrt(Idx)", lambda: math.isqrt(Idx(16)))
show("factorial(float)", lambda: math.factorial(1.5))
show("nextafter(steps=Idx)", lambda: math.nextafter(1.0, 2.0, steps=Idx(2)))

# ---- sys.setrecursionlimit ----
import sys

_old = sys.getrecursionlimit()
sys.setrecursionlimit(Idx(3000))
print("setrecursionlimit(Idx) =", sys.getrecursionlimit())
sys.setrecursionlimit(_old)
show("setrecursionlimit(float)", lambda: sys.setrecursionlimit(1.5))

# ---- Unicode*Error start/end ----
_ude = UnicodeDecodeError("utf-8", b"xxxxx", Idx(1), Idx(3), "reason")
print("UnicodeDecodeError start/end =", _ude.start, _ude.end, type(_ude.start).__name__)
_uee = UnicodeEncodeError("utf-8", "xxxxx", Idx(0), Idx(2), "reason")
print("UnicodeEncodeError start/end =", _uee.start, _uee.end)
_ute = UnicodeTranslateError("xxxxx", Idx(1), Idx(4), "reason")
print("UnicodeTranslateError start/end =", _ute.start, _ute.end)
show("UnicodeDecodeError(float)", lambda: UnicodeDecodeError("utf-8", b"x", 1.0, 1, "r"))
