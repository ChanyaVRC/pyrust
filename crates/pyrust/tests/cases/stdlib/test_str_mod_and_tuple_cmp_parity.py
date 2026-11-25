"""CPython 3.12 parity tests for str/bytes.__mod__ tuple subclasses & tuple richcmp (issue #3206)."""

import collections
import pprint


# ── 1. str.__mod__ & bytes.__mod__ unpacking tuple subclasses ────────────────
class MyTuple(tuple):
    pass


class MappingLikeTuple(tuple):
    # Even with a custom __getitem__, CPython treats tuple subclasses as
    # positional tuples, never as mappings.
    def __getitem__(self, key):
        return super().__getitem__(key)


assert "%s %s" % MyTuple((1, 2)) == "1 2"
assert "%d-%s" % MyTuple((42, "ok")) == "42-ok"
assert "%s" % MyTuple(("only",)) == "only"
assert "no args" % MyTuple(()) == "no args"
assert "%s %s" % MappingLikeTuple(("a", "b")) == "a b"

# bytes / bytearray formatting with tuple subclasses
assert b"%s %d" % MyTuple((b"hi", 7)) == b"hi 7"
assert bytearray(b"%s=%d") % MyTuple((b"k", 3)) == bytearray(b"k=3")

# Arity errors still match CPython
try:
    "%s" % MyTuple((1, 2))
    assert False
except TypeError:
    pass

try:
    "%s %s" % MyTuple((1,))
    assert False
except TypeError:
    pass

try:
    "%s" % MyTuple(())
    assert False
except TypeError:
    pass

# collections.namedtuple.__repr__ uses `% self` directly
Point = collections.namedtuple("Point", ["x", "y"])
pt = Point(10, 20)
assert repr(pt) == "Point(x=10, y=20)"
assert "%d, %d" % pt == "10, 20"

print("str/bytes mod tuple subclass ok")


# ── 2. tuple `<` / rich-comparison edge cases ────────────────────────────────
class CustomOrder:
    def __init__(self, v):
        self.v = v

    def __lt__(self, other):
        return self.v < other.v


# Plain tuple containing objects that define only __lt__
assert (CustomOrder(1),) < (CustomOrder(2),)
assert not ((CustomOrder(2),) < (CustomOrder(1),))
assert (CustomOrder(1), CustomOrder(2)) < (CustomOrder(2), CustomOrder(1))
assert ((CustomOrder(1),),) < ((CustomOrder(2),),)

# Direct tuple.__lt__ slot
assert tuple.__lt__((CustomOrder(1),), (CustomOrder(2),)) is True
assert tuple.__lt__((CustomOrder(2),), (CustomOrder(1),)) is False

# sorted / list.sort / min / max with tuple keys containing custom __lt__ objects
items = [(3, "c"), (1, "a"), (2, "b")]
assert sorted(items, key=lambda t: (CustomOrder(t[0]), CustomOrder(t[1]))) == [
    (1, "a"),
    (2, "b"),
    (3, "c"),
]
copy_items = list(items)
copy_items.sort(key=lambda t: (CustomOrder(t[0]), CustomOrder(t[1])))
assert copy_items == [(1, "a"), (2, "b"), (3, "c")]
assert min(items, key=lambda t: (CustomOrder(t[0]),)) == (1, "a")
assert max(items, key=lambda t: (CustomOrder(t[0]),)) == (3, "c")

# pprint with heterogeneous unorderable dict keys (uses plain _safe_tuple returning 2-tuple of _safe_key)
mixed_dict = {1: "int", "a": "str", None: "none"}
formatted = pprint.pformat(mixed_dict)
assert "1: 'int'" in formatted and "'a': 'str'" in formatted and "None: 'none'" in formatted

# Tuple subclasses with & without custom __lt__/__gt__ returning NotImplemented
assert MyTuple((1, 2)) < MyTuple((1, 3))
assert MyTuple((CustomOrder(1),)) < MyTuple((CustomOrder(2),))


class NoOrderTuple(tuple):
    def __lt__(self, other):
        return NotImplemented

    def __gt__(self, other):
        return NotImplemented


try:
    _ = NoOrderTuple((1,)) < NoOrderTuple((2,))
    assert False, "expected TypeError when both __lt__ and __gt__ return NotImplemented"
except TypeError:
    pass


class PartialTuple(tuple):
    def __lt__(self, other):
        return NotImplemented


# Falls back to inherited tuple.__gt__
assert PartialTuple((1,)) < PartialTuple((2,))
assert PartialTuple((1,)) < (2,)

print("tuple richcmp ok")
print("test_str_mod_and_tuple_cmp_parity ok")
