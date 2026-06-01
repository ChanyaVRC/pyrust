# list.sort() (no key) must dispatch user comparison dunders, matching
# CPython.  Previously the no-key path ran in the interpreter-free builtins
# crate, which could not reach user `__lt__` and raised a spurious TypeError
# (#1925).


# Only __lt__ defined: sorts using it.
class Lt:
    def __init__(self, v):
        self.v = v

    def __lt__(self, other):
        return self.v < other.v


xs = [Lt(3), Lt(1), Lt(2)]
xs.sort()
print([c.v for c in xs])

# reverse=True
xs.sort(reverse=True)
print([c.v for c in xs])


# Only __gt__ defined: CPython uses the reflected __lt__ (a < b via b.__gt__(a)).
class Gt:
    def __init__(self, v):
        self.v = v

    def __gt__(self, other):
        return self.v > other.v


ys = [Gt(3), Gt(1), Gt(2)]
ys.sort()
print([c.v for c in ys])


# Stability: equal elements keep their input order.
class Pair:
    def __init__(self, k, tag):
        self.k = k
        self.tag = tag

    def __lt__(self, other):
        return self.k < other.k


ps = [Pair(1, "a"), Pair(1, "b"), Pair(0, "c"), Pair(1, "d")]
ps.sort()
print([(p.k, p.tag) for p in ps])


# No ordering dunder: exact CPython TypeError.
class NoCmp:
    pass


zs = [NoCmp(), NoCmp()]
try:
    zs.sort()
except TypeError as e:
    print("TypeError:", e)


# __lt__ raising mid-sort propagates.
class Boom:
    def __init__(self, v):
        self.v = v

    def __lt__(self, other):
        raise ValueError("boom")


bs = [Boom(1), Boom(2)]
try:
    bs.sort()
except ValueError as e:
    print("ValueError:", e)


# Bound-method form (xs.sort grabbed then called) takes the same path.
ms = [Lt(2), Lt(0), Lt(1)]
m = ms.sort
m()
print([c.v for c in ms])


# Primitive lists are unaffected (fast path).
ints = [3, 1, 2]
ints.sort()
print(ints)
ints.sort(reverse=True)
print(ints)

strs = ["banana", "apple", "cherry"]
strs.sort()
print(strs)
