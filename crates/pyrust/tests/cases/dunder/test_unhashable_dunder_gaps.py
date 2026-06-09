# Issue #2291: niche descriptor / dunder gaps that remained after #2266/#2276.
# Every assertion below is verified byte-for-byte against python3.12.
#
#   1. The unhashable built-in types set `__hash__ = None` on the *type*, so
#      `list.__hash__` (the class attribute) is `None`, and calling it raises
#      `'NoneType' object is not callable` — not a descriptor message.
#   2. Passing a keyword argument to a *slot-wrapper* descriptor reports the
#      anonymous "wrapper <name>() takes no keyword arguments" form (the
#      `__format__` method_descriptor keeps the type-qualified form).


def show(label, fn):
    try:
        fn()
    except TypeError as e:
        print(label, str(e))


# === (1) list/dict/set/bytearray.__hash__ is None (the attribute) ============
print("list.__hash__", list.__hash__)
print("dict.__hash__", dict.__hash__)
print("set.__hash__", set.__hash__)
print("bytearray.__hash__", bytearray.__hash__)

# Calling the None attribute raises 'NoneType' object is not callable.
show("list.__hash__()", lambda: list.__hash__())
show("dict.__hash__()", lambda: dict.__hash__())
show("set.__hash__()", lambda: set.__hash__())
show("bytearray.__hash__()", lambda: bytearray.__hash__())

# A subclass that does NOT override __hash__ inherits None; one that does keeps
# its own callable __hash__.
class L(list):
    pass


print("L.__hash__", L.__hash__)


class H(list):
    def __hash__(self):
        return 7


print("H().__hash__()", H().__hash__())

# Hashable primitives still expose a working __hash__.
print("str.__hash__('x') type", type(str.__hash__("x")).__name__)
print("int.__hash__(5)", int.__hash__(5))
print("tuple.__hash__ callable", callable(tuple.__hash__))


# === (2) kwarg to a slot-wrapper descriptor: "wrapper <name>() ..." ==========
# Slot wrappers (anonymous): __hash__/__repr__/__str__/__len__/__lt__/__add__/…
show("str.__hash__ kw", lambda: str.__hash__("x", k=1))
show("str.__repr__ kw", lambda: str.__repr__("x", k=1))
show("str.__str__ kw", lambda: str.__str__("x", k=1))
show("int.__hash__ kw", lambda: int.__hash__(5, k=1))
show("tuple.__hash__ kw", lambda: tuple.__hash__((1,), k=1))
show("str.__len__ kw", lambda: str.__len__("a", k=1))
show("list.__len__ kw", lambda: list.__len__([1], k=1))
show("list.__add__ kw", lambda: list.__add__([1], [2], k=1))
show("str.__getitem__ kw", lambda: str.__getitem__("a", 0, k=1))
show("list.__contains__ kw", lambda: list.__contains__([1], 1, k=1))
show("set.__or__ kw", lambda: set.__or__(set(), set(), k=1))

# Named method-wrappers (mp_subscript / sq_contains): "<type>.<name>() ..."
show("list.__getitem__ kw", lambda: list.__getitem__([1, 2], 0, k=1))
show("dict.__getitem__ kw", lambda: dict.__getitem__({1: 2}, 1, k=1))
show("dict.__contains__ kw", lambda: dict.__contains__({}, 1, k=1))
show("set.__contains__ kw", lambda: set.__contains__({1}, 1, k=1))
show("frozenset.__contains__ kw", lambda: frozenset.__contains__(frozenset({1}), 1, k=1))

# __format__ is a method_descriptor: keeps the type-qualified wording.
show("str.__format__ kw", lambda: str.__format__("x", "", k=1))
show("int.__format__ kw", lambda: int.__format__(5, "", k=1))

# === (4) complex.__format__ empty spec renders str(complex) ==================
print("format(1+2j, '')", format(1 + 2j, ""))
print("(1+2j).__format__('')", (1 + 2j).__format__(""))
print("(3-4j).__format__('')", (3 - 4j).__format__(""))
