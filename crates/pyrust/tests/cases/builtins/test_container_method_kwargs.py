"""Issue #2500: dict / set / frozenset / tuple / bytes / bytearray
method_descriptors that accept no keyword arguments must raise
``TypeError: <type>.<method>() takes no keyword arguments`` rather than
silently dropping the unexpected keyword (CPython 3.12 parity).

Methods that DO take keywords (``dict.update``, ``list.sort``, ``bytes.split``,
``bytes.decode``, …) must keep working.
"""


def show(label, fn):
    try:
        fn()
        print(label, "-> NO ERROR")
    except TypeError as e:
        print(label, "->", e)


# --- bound method calls (the issue's repros) ---------------------------------
show("dict.get", lambda: {1: 2}.get(1, default=None))
show("dict.pop", lambda: {1: 2}.pop(1, default=None))
show("dict.setdefault", lambda: {1: 2}.setdefault(1, default=None))
show("dict.popitem", lambda: {1: 2}.popitem(x=1))
show("dict.copy", lambda: {1: 2}.copy(x=1))
show("dict.keys", lambda: {1: 2}.keys(x=1))
show("dict.values", lambda: {1: 2}.values(x=1))
show("dict.items", lambda: {1: 2}.items(x=1))
show("set.add", lambda: {1}.add(1, x=1))
show("set.discard", lambda: {1}.discard(1, x=1))
show("set.difference", lambda: {1}.difference({1}, x=1))
show("set.union", lambda: {1}.union({1}, x=1))
show("set.symmetric_difference", lambda: {1}.symmetric_difference({1}, x=1))
show("set.isdisjoint", lambda: {1}.isdisjoint({1}, x=1))
show("frozenset.isdisjoint", lambda: frozenset([1]).isdisjoint({1}, x=1))
show("frozenset.union", lambda: frozenset([1]).union({1}, x=1))
show("frozenset.intersection", lambda: frozenset([1]).intersection({1}, x=1))
show("frozenset.copy", lambda: frozenset([1]).copy(x=1))
show("tuple.count", lambda: (1, 2).count(1, x=1))
show("tuple.index", lambda: (1, 2).index(1, x=1))
show("bytes.find", lambda: b"hi".find(b"h", end=5))
show("bytes.count", lambda: b"hi".count(b"h", end=5))
show("bytes.join", lambda: b",".join([b"a"], x=1))
show("bytearray.find", lambda: bytearray(b"hi").find(b"h", end=5))
show("bytearray.count", lambda: bytearray(b"hi").count(b"h", x=1))


# --- unbound (type-level) calls ----------------------------------------------
show("dict.get unbound", lambda: dict.get({1: 2}, 1, default=None))
show("set.add unbound", lambda: set.add({1}, 1, x=1))
show("frozenset.isdisjoint unbound",
     lambda: frozenset.isdisjoint(frozenset([1]), {1}, x=1))
show("tuple.count unbound", lambda: tuple.count((1, 2), 1, x=1))
show("bytes.find unbound", lambda: bytes.find(b"hi", b"h", end=5))


# --- builtin-subclass instances reject with the base type's wording ----------
class MyDict(dict):
    pass


class MySet(set):
    pass


class MyTuple(tuple):
    pass


show("MyDict().get", lambda: MyDict({1: 2}).get(1, default=None))
show("MySet().add", lambda: MySet([1]).add(1, x=1))
show("MyTuple().count", lambda: MyTuple((1, 2)).count(1, x=1))


# --- unknown attribute still resolves to AttributeError first ----------------
def show_attr(label, fn):
    try:
        fn()
        print(label, "-> NO ERROR")
    except AttributeError as e:
        print(label, "->", e)


show_attr("dict.nonexistent", lambda: {1: 2}.nonexistent(z=1))
show_attr("bytes.nonexistent", lambda: b"x".nonexistent(z=1))
show_attr("frozenset.add (frozenset has no add)",
          lambda: frozenset([1]).add(1, z=1))


# --- keyword-accepting methods keep working ----------------------------------
d = {}
d.update(a=1, b=2)
print("dict.update kwargs:", d)

lst = [3, 1, 2]
lst.sort(reverse=True)
print("list.sort reverse:", lst)

print("bytes.split maxsplit:", b"a b c".split(maxsplit=1))
print("bytes.decode encoding:", b"hi".decode(encoding="ascii"))
print("bytearray.split maxsplit:", bytearray(b"a b c").split(maxsplit=1))
