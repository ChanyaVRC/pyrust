# Issue #1936: int/str/float/bytes/tuple/frozenset subclass instances hash by
# their backing value, so they key dicts/sets interchangeably with the base.
class I(int):
    pass


class S(str):
    pass


class F(float):
    pass


class B(bytes):
    pass


class T(tuple):
    pass


print(hash(I(5)) == hash(5))
print(hash(S("x")) == hash("x"))
print(hash(F(1.5)) == hash(1.5))
print(hash(B(b"x")) == hash(b"x"))
print(hash(T((1, 2))) == hash((1, 2)))

# Dict keying is interchangeable in both directions.
print({I(1): "a"}[1])
print({1: "a"}[I(1)])
print({S("k"): "v"}["k"])
print({T((1, 2)): "v"}[(1, 2)])

# Set membership and dedup.
print(I(1) in {1, 2})
print(1 in {I(1), I(2)})
print(len({1, I(1)}))
print(len({b"x", B(b"x")}))

# A user __hash__ override is used.
class IHash(int):
    def __hash__(self):
        return 999


print(hash(IHash(5)))

# A subclass that sets __hash__ = None stays unhashable.
class INone(int):
    __hash__ = None


try:
    hash(INone(5))
    print("hashable")
except TypeError:
    print("unhashable")
