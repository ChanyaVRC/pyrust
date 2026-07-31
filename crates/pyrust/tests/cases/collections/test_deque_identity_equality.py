# `collections.deque` compares elements with CPython's identity-then-equality
# rule (`PyObject_RichCompareBool`: `x is y or x == y`), like list/tuple do
# (#2344 / #2535 / #2911).  NaN is the observable case: a NaN must find itself
# even though `nan == nan` is False, and must never find a *different* NaN.

from collections import deque

a = float("nan")
b = float("nan")

d = deque([1, a, 2])

# The same object finds itself.
print(a in d, d.index(a), d.count(a))

# A distinct NaN object does not.
print(b in d, d.count(b))
try:
    d.index(b)
except ValueError as e:
    print("ValueError:", e)
try:
    deque([1, a, 2]).remove(b)
except ValueError as e:
    print("ValueError:", e)

# remove() targets exactly the identical object.
d2 = deque([1, a, b, 2])
d2.remove(a)
print(len(d2), d2[1] is b, list(d2)[0], list(d2)[2])

# index() honours the start/stop window while still using identity.
d3 = deque([a, 1, a, 2])
print(d3.index(a), d3.index(a, 1), d3.index(a, 1, 3))
try:
    d3.index(a, 3)
except ValueError as e:
    print("ValueError:", e)

# count() over repeats of the same and of distinct NaNs.
print(deque([a, a]).count(a), deque([a, b]).count(a), deque([a, b]).count(b))

# deque == deque is element-wise identity-then-equality.
print(deque([a]) == deque([a]), deque([a]) == deque([b]))
print(deque([a]) != deque([a]), deque([a]) != deque([b]))
print(deque([1, a, 2]) == deque([1, a, 2]), deque([1, a]) == deque([1, b]))
print(deque([a]) == deque([a, a]), deque([]) == deque([]))

# A NaN nested one level down still resolves through the element's own
# identity-then-equality comparison.
print(deque([[a]]) == deque([[a]]), deque([[a]]) == deque([[b]]))
print(deque([(a,)]) == deque([(a,)]), deque([(a,)]) == deque([(b,)]))
print([a] in deque([[a]]), [b] in deque([[a]]))

# A NaN-bearing complex is an object too: it finds itself, not its twin.
za = complex(float("nan"), 1.0)
zb = complex(float("nan"), 1.0)
print(za in deque([za]), zb in deque([za]))
print(deque([za]) == deque([za]), deque([za]) == deque([zb]))
print(deque([za]).index(za), deque([za]).count(za), deque([za]).count(zb))

# A bounded deque behaves the same.
bd = deque([a], maxlen=3)
print(a in bd, bd.index(a), bd.count(a), deque([a], maxlen=3) == deque([a]))


# Identity wins over a user __eq__ that answers False for everything.
class Never:
    def __eq__(self, other):
        return False

    def __hash__(self):
        return 0


w = Never()
v = Never()
dw = deque([w])
print(w in dw, dw.index(w), dw.count(w))
print(v in dw, dw.count(v))
print(deque([w]) == deque([w]), deque([w]) == deque([v]))
dw.remove(w)
print(len(dw))

# Non-NaN elements are unaffected.
print(1 in deque([1]), 3 in deque([1, 2]))
print(deque([1, 2]) == deque([1, 2]), deque([1, 2]) == deque([1, 3]))
print(deque([1, 2, 1]).count(1), deque([1, 2, 1]).index(1, 1))
print(deque(["x"]).index("x"), "x" in deque(["x"]))
print(0.0 in deque([-0.0]), deque([0.0]) == deque([-0.0]))
inf = float("inf")
print(inf in deque([float("inf")]), deque([inf]) == deque([float("inf")]))

# Bare NaN equality is untouched.
print(a == a, a == b, a != a)
