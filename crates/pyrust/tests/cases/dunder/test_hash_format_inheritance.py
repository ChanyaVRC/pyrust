# Issue #2299: hash/format dunder *inheritance* gaps left after #2296.
# Every assertion below is verified byte-for-byte against python3.12.
#
#   1. A subclass of an unhashable builtin (list/dict/set/bytearray) that does
#      NOT override __hash__ inherits __hash__ = None, so hash(instance) raises
#      `unhashable type: '<subclass>'` — the instance-hash path must honour the
#      inherited None, not fall back to an identity hash.
#   2. `object.__format__` (and the inherited `bytes.__format__`) take no
#      keyword arguments; both resolve to the same slot, so the error always
#      names `object.__format__()`.


def show(label, fn):
    try:
        v = fn()
    except TypeError as e:
        print(label, "TypeError:", e)
    else:
        print(label, "->", v)


# === (1) hash() of an unhashable-builtin subclass ============================
class L(list):
    pass


class D(dict):
    pass


class S(set):
    pass


class BA(bytearray):
    pass


show("hash(L())", lambda: hash(L()))
show("hash(D())", lambda: hash(D()))
show("hash(S())", lambda: hash(S()))
show("hash(BA())", lambda: hash(BA()))

# Indirect (multi-level) inheritance still stays unhashable.
class L2(L):
    pass


show("hash(L2())", lambda: hash(L2()))


# Multiple inheritance follows the C3 MRO: the first class to supply __hash__
# wins.  `(list, M)` keeps the builtin's None (unhashable); `(M, list)` resolves
# M.__hash__ first (hashable).
class M:
    def __hash__(self):
        return 99


class C1(list, M):
    pass


class C2(M, list):
    pass


show("hash(C1())", lambda: hash(C1()))
print("hash(C2())", hash(C2()))


# A subclass that DEFINES __hash__ is hashable and uses its own value.
class H(list):
    def __hash__(self):
        return 7


print("hash(H())", hash(H()))


# A subclass that re-enables hashing by setting __hash__ explicitly.
class R(list):
    __hash__ = object.__hash__


print("type(hash(R()))", type(hash(R())).__name__)

# Operator path for the bare builtin is unchanged.
show("hash([])", lambda: hash([]))
show("hash({})", lambda: hash({}))

# Normal hashables are unaffected.
print("hash(()) ", hash(()))
print("hash(frozenset())==hash(frozenset())", hash(frozenset()) == hash(frozenset()))
print("hash('x') type", type(hash("x")).__name__)
print("hash(5)", hash(5))
print("hash(object()) is int", isinstance(hash(object()), int))


# === (2) object/bytes.__format__ reject keyword arguments ====================
show("bytes.__format__ kw", lambda: bytes.__format__(b"", "", k=1))
show("object.__format__ kw", lambda: object.__format__(object(), "", k=1))
show("bound bytes kw", lambda: b"".__format__("", k=1))
show("bound object kw", lambda: object().__format__("", k=1))

# Happy paths: no kwargs still format.
print("bytes.__format__(b'hi','')", bytes.__format__(b"hi", ""))
print("object.__format__ ok", isinstance(object.__format__(object(), ""), str))
print("format(b'hi')", format(b"hi"))
print("format(object()) is str", isinstance(format(object()), str))
