# Issue #1909: built-in container/sequence protocol dunders are exposed as
# bound method-wrappers on the primitive types, consistent with hasattr/dir,
# dispatching through the same machinery as the implicit operators.


# --- bound-method call form (obj.__dunder__(...)) ---------------------------
print([1, 2, 3].__len__())
print([1, 2, 3].__getitem__(1))
print([1, 2, 3].__getitem__(slice(0, 2)))
print([1, 2].__add__([3]))
print([1, 2].__mul__(2))
print([1, 2, 3].__contains__(2))
print("abc".__getitem__(1))
print("abc".__contains__("b"))
print("ab".__add__("cd"))
print("ab".__mul__(2))
print((1, 2).__contains__(1))
print((1, 2).__add__((3,)))
print((1, 2).__getitem__(0))
print({1, 2}.__contains__(1))
print({1, 2}.__len__())
print({1: 2}.__contains__(1))
print({1: 2}.__getitem__(1))
print(frozenset({1, 2}).__contains__(1))
print(b"ab".__getitem__(0))
print(b"ab".__add__(b"cd"))
print(b"ab".__mul__(2))
print(b"ab".__contains__(97))

# --- mutation via dunder ----------------------------------------------------
lst = [1, 2, 3]
lst.__setitem__(0, 9)
print(lst)
lst.__delitem__(0)
print(lst)

d = {}
d.__setitem__("k", 1)
print(d)
d.__delitem__("k")
print(d)

ba = bytearray(b"ab")
print(ba.__getitem__(0))
ba.__setitem__(0, 99)
print(ba)
ba.__delitem__(0)
print(ba)
print(ba.__len__())

# --- unbound type-level form (Type.__dunder__(obj, ...)) --------------------
l2 = [1, 2]
list.__setitem__(l2, 0, 9)
print(l2)
print(list.__add__([1], [2]))
print(list.__getitem__([1, 2, 3], 1))
print(dict.__getitem__({1: 2}, 1))
print(str.__len__("abc"))
print(str.__getitem__("abc", 1))
print(tuple.__contains__((1, 2), 1))

# unbound form with wrong-type receiver raises the descriptor TypeError
try:
    list.__len__("abc")
except TypeError as e:
    print("TypeError", e)

# --- equality with the operator forms ---------------------------------------
print([1, 2].__add__([3]) == [1, 2] + [3])
print({1, 2}.__contains__(1) is (1 in {1, 2}))
print("abc".__getitem__(1) == "abc"[1])
print([1, 2, 3].__len__() == len([1, 2, 3]))


# --- __mul__ uses the __index__ protocol (stricter than `*`) -----------------
class Idx:
    def __index__(self):
        return 2


print([1, 2].__mul__(Idx()))
try:
    [1, 2].__mul__("x")
except TypeError as e:
    print("TypeError", e)


class Plain:
    pass


try:
    [1, 2].__mul__(Plain())
except TypeError as e:
    print("TypeError", e)

# --- genuinely-missing dunders raise AttributeError (no RuntimeError leak) ---
for obj, name in [
    (set(), "__getitem__"),
    ({}, "__add__"),
    ("", "__setitem__"),
    ((), "__setitem__"),
    (frozenset(), "__add__"),
]:
    try:
        getattr(obj, name)
    except AttributeError as e:
        print("AttributeError", e)

# calling a genuinely-missing dunder via the method-call form
try:
    set().__getitem__(0)
except AttributeError as e:
    print("AttributeError", e)

# --- hasattr / dir / access are mutually consistent -------------------------
objs = {
    "list": [1, 2, 3],
    "tuple": (1, 2, 3),
    "str": "abc",
    "bytes": b"abc",
    "bytearray": bytearray(b"abc"),
    "dict": {1: 2},
    "set": {1, 2},
    "frozenset": frozenset({1, 2}),
}
dunders = [
    "__len__",
    "__getitem__",
    "__setitem__",
    "__delitem__",
    "__contains__",
    "__add__",
    "__mul__",
    "__iter__",
]
for tn, obj in objs.items():
    for dn in dunders:
        present = hasattr(obj, dn)
        in_dir = dn in dir(obj)
        accessible = present
        try:
            getattr(obj, dn)
        except AttributeError:
            accessible = False
        # all three must agree
        print(tn, dn, present, present == in_dir == accessible)

# --- KeyError / IndexError propagate from the dunder forms ------------------
try:
    {}.__getitem__("missing")
except KeyError as e:
    print("KeyError", e)
try:
    [1, 2].__getitem__(9)
except IndexError as e:
    print("IndexError", e)
try:
    [1].__setitem__(9, 0)
except IndexError as e:
    print("IndexError", e)


# --- builtin subclasses route protocol dunders to the backing primitive -----
class MyList(list):
    pass


ml = MyList([1, 2, 3])
print(ml.__len__())
print(ml.__getitem__(1))
print(ml.__contains__(2))
print(ml.__add__([4]))


class MyDict(dict):
    pass


md = MyDict({"a": 1})
print(md.__getitem__("a"))
print(md.__len__())
print(md.__contains__("a"))
