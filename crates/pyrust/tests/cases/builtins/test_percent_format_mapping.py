# %-format accepts any mapping and honours __missing__ (issue #2089)
# CPython 3.12 parity for both str and bytes %-formatting.

# Plain dict (unregressed fast path).
print("%(a)s" % {"a": 5})
print("%(a)s-%(b)d %%lit" % {"a": "x", "b": 7})


# dict subclass.
class D(dict):
    pass


print("%(a)s" % D(a=5))
print("%(a)s-%(b)d" % D(a="x", b=7))


# dict subclass with __missing__.
class DM(dict):
    def __missing__(self, k):
        return f"<{k}>"


print("%(z)s" % DM(a=1))
print("%(a)s/%(z)s" % DM(a=1))

# collections.defaultdict consults its factory via __missing__.
from collections import defaultdict

print("%(missing)s" % defaultdict(lambda: "DEF"))


# A custom mapping (only __getitem__) is accepted.
class G:
    def __getitem__(self, k):
        return f"got-{k}"


print("%(a)s" % G())

# Plain dict missing key still raises KeyError.
try:
    "%(z)s" % {"a": 1}
except KeyError as e:
    print("KeyError", e)


# An object without __getitem__ is not a mapping.
class H:
    pass


try:
    "%(a)s" % H()
except TypeError as e:
    print(e)

# bytes %-format: mapping with bytes keys, subclass, and __missing__.
print(b"%(a)s" % {b"a": b"hi"})
print(b"%(a)s" % D({b"a": b"hi"}))


class DMB(dict):
    def __missing__(self, k):
        return b"<m>"


print(b"%(z)s" % DMB({b"a": b"x"}))
