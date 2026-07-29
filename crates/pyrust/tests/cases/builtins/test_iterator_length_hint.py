# `operator.length_hint` / `__length_hint__` parity (issue #2920).
#
# CPython gives its concrete built-in iterators a `__length_hint__` slot that
# reports the remaining element count, and `operator.length_hint` (the
# accelerated `_operator` one, which shadows the pure-Python definition) reads
# it through `PyObject_LengthHint`.  This fixture pins:
#   * which iterator types expose the slot and which do not;
#   * the remaining count fresh / partly consumed / exhausted;
#   * the mutation rules — sequence walks re-read the live length, dict and set
#     cursors collapse to zero once the container's *size* moved, and a cursor
#     that already raised keeps reporting zero;
#   * the `len()` -> `__length_hint__` -> default fallback chain and every
#     error class it can raise;
#   * the `length_hint(obj, default)` signature, including the `__index__`
#     coercion of `default` and the positional-only argument list.

import operator
from collections import OrderedDict, deque


def show(label, fn):
    try:
        print(label, repr(fn()))
    except BaseException as e:
        print(label, type(e).__name__ + ":", str(e))


def probe(label, make):
    it = make()
    print(
        label,
        "hasattr=%s" % hasattr(it, "__length_hint__"),
        "hint=%s" % operator.length_hint(it),
    )


print("--- which iterator types expose the slot ---")
probe("list_iter", lambda: iter([1, 2, 3]))
probe("list_revit", lambda: reversed([1, 2, 3]))
probe("tuple_iter", lambda: iter((1, 2, 3)))
probe("tuple_revit", lambda: reversed((1, 2, 3)))
probe("str_iter", lambda: iter("abc"))
probe("str_revit", lambda: reversed("abc"))
probe("str_wide_iter", lambda: iter("aé漢"))
probe("bytes_iter", lambda: iter(b"abc"))
probe("bytes_revit", lambda: reversed(b"abc"))
probe("bytearray_iter", lambda: iter(bytearray(b"abc")))
probe("range_iter", lambda: iter(range(10)))
probe("range_step_iter", lambda: iter(range(0, 10, 3)))
probe("range_neg_iter", lambda: iter(range(10, 0, -1)))
probe("range_revit", lambda: reversed(range(10)))
probe("range_empty_iter", lambda: iter(range(5, 5)))
probe("dict_keyiter", lambda: iter({1: 1, 2: 2, 3: 3}))
probe("dict_valueiter", lambda: iter({1: 1, 2: 2, 3: 3}.values()))
probe("dict_itemiter", lambda: iter({1: 1, 2: 2, 3: 3}.items()))
probe("dict_revkeyiter", lambda: reversed({1: 1, 2: 2, 3: 3}))
probe("dict_revvalueiter", lambda: reversed({1: 1, 2: 2, 3: 3}.values()))
probe("dict_revitemiter", lambda: reversed({1: 1, 2: 2, 3: 3}.items()))
probe("set_iter", lambda: iter({1, 2, 3}))
probe("frozenset_iter", lambda: iter(frozenset({1, 2, 3})))
probe("deque_iter", lambda: iter(deque([1, 2, 3])))
probe("dict_subclass_iter", lambda: iter(type("D", (dict,), {})(a=1, b=2)))
# CPython's `odict_iterator` is the one built-in mapping cursor with no slot.
probe("odict_iter", lambda: iter(OrderedDict(a=1, b=2, c=3)))
probe("odict_view_iter", lambda: iter(OrderedDict(a=1, b=2, c=3).items()))
probe("odict_revit", lambda: reversed(OrderedDict(a=1, b=2, c=3)))
# Adapters and generators have no hint at all.
probe("enumerate", lambda: enumerate([1, 2, 3]))
probe("map", lambda: map(str, [1, 2, 3]))
probe("filter", lambda: filter(None, [1, 2, 3]))
probe("zip", lambda: zip([1, 2, 3], [4, 5, 6]))
probe("generator", lambda: (x for x in [1, 2, 3]))
probe("callable_iter", lambda: iter(lambda: 1, 1))

print("--- containers themselves have no slot but do have len() ---")
probe("list", lambda: [1, 2, 3])
probe("str", lambda: "abc")
probe("dict_keys_view", lambda: {1: 1, 2: 2}.keys())
probe("range", lambda: range(10))
probe("frozenset", lambda: frozenset({1, 2}))

print("--- fresh / partly consumed / exhausted ---")
for label, make in [
    ("list", lambda: iter([1, 2, 3])),
    ("list_rev", lambda: reversed([1, 2, 3])),
    ("tuple", lambda: iter((1, 2, 3))),
    ("str", lambda: iter("abc")),
    ("bytes", lambda: iter(b"abc")),
    ("range", lambda: iter(range(3))),
    ("range_rev", lambda: reversed(range(3))),
    ("dict", lambda: iter({1: 1, 2: 2, 3: 3})),
    ("dict_items", lambda: iter({1: 1, 2: 2, 3: 3}.items())),
    ("set", lambda: iter({1, 2, 3})),
]:
    it = make()
    counts = [operator.length_hint(it)]
    for _ in range(3):
        next(it)
        counts.append(operator.length_hint(it))
    show(label + "-drain", lambda it=it: next(it))
    counts.append(operator.length_hint(it))
    print(label, counts)

print("--- the slot is callable directly ---")
it = iter([1, 2, 3, 4])
next(it)
show("direct", lambda: it.__length_hint__())
show("direct-arg", lambda: it.__length_hint__(1))
show("range-direct", lambda: iter(range(7)).__length_hint__())
show("map-direct", lambda: map(str, [1]).__length_hint__())
show("gen-direct", lambda: (x for x in []).__length_hint__())

print("--- sequence walks re-read the live length ---")
data = [1, 2, 3]
it = iter(data)
next(it)
data.extend([4, 5])
show("list-grown", lambda: operator.length_hint(it))
data = [1, 2, 3, 4, 5]
it = iter(data)
for _ in range(4):
    next(it)
del data[1:]
show("list-truncated", lambda: operator.length_hint(it))
show("list-truncated-next", lambda: next(it))
show("list-truncated-after-stop", lambda: operator.length_hint(it))

data = [1, 2, 3, 4, 5]
rev = reversed(data)
next(rev)
show("rev-fresh", lambda: operator.length_hint(rev))
del data[1:]
show("rev-truncated", lambda: operator.length_hint(rev))
data = [1, 2, 3]
rev = reversed(data)
next(rev)
data.append(4)
show("rev-grown", lambda: operator.length_hint(rev))

print("--- dict/set cursors watch the container size ---")
d = {1: 1, 2: 2, 3: 3}
it = iter(d)
next(it)
show("dict-partial", lambda: operator.length_hint(it))
d[4] = 4
show("dict-after-insert", lambda: operator.length_hint(it))

d = {1: 1, 2: 2, 3: 3}
it = iter(d)
next(it)
del d[3]
show("dict-after-delete", lambda: operator.length_hint(it))
show("dict-next-raises", lambda: next(it))
# The #2915 latch: the error repeats, and the hint stays zero with it.
show("dict-hint-after-latch", lambda: operator.length_hint(it))
show("dict-next-again", lambda: next(it))
show("dict-hint-still-latched", lambda: operator.length_hint(it))

s = {1, 2, 3}
it = iter(s)
next(it)
show("set-partial", lambda: operator.length_hint(it))
s.add(9)
show("set-after-add", lambda: operator.length_hint(it))
show("set-next-raises", lambda: next(it))
show("set-hint-after-latch", lambda: operator.length_hint(it))

# A same-size swap leaves the recorded quota intact.
d = {1: 1, 2: 2, 3: 3}
it = iter(d)
next(it)
del d[2]
d[7] = 7
show("dict-same-size-swap", lambda: operator.length_hint(it))
s = {1, 2, 3}
it = iter(s)
next(it)
s.discard(1)
s.add(99)
show("set-same-size-swap", lambda: operator.length_hint(it))
# Replacing a value never changes the size.
d = {1: 1, 2: 2}
it = iter(d.items())
next(it)
d[1] = 99
show("dict-value-replaced", lambda: operator.length_hint(it))

# Dict view and reverse cursors follow the same rule.
d = {1: 1, 2: 2, 3: 3}
it = iter(d.keys())
next(it)
show("view-partial", lambda: operator.length_hint(it))
d[9] = 9
show("view-after-insert", lambda: operator.length_hint(it))
d = {1: 1, 2: 2, 3: 3}
it = reversed(d)
next(it)
show("dictrev-partial", lambda: operator.length_hint(it))
d[9] = 9
show("dictrev-after-insert", lambda: operator.length_hint(it))

print("--- legacy __len__ + __getitem__ sequences ---")


class Seq:
    def __len__(self):
        return 3

    def __getitem__(self, i):
        if i >= 3:
            raise IndexError
        return i


class NoLenSeq:
    def __getitem__(self, i):
        if i >= 3:
            raise IndexError
        return i


it = iter(Seq())
show("seqiter-hasattr", lambda: hasattr(it, "__length_hint__"))
show("seqiter-fresh", lambda: operator.length_hint(it))
next(it)
show("seqiter-partial", lambda: operator.length_hint(it))
it = reversed(Seq())
show("revseq-hasattr", lambda: hasattr(it, "__length_hint__"))
show("revseq-fresh", lambda: operator.length_hint(it))
next(it)
show("revseq-partial", lambda: operator.length_hint(it))
it = iter(NoLenSeq())
show("nolen-hasattr", lambda: hasattr(it, "__length_hint__"))
show("nolen-direct", lambda: it.__length_hint__())
show("nolen-hint", lambda: operator.length_hint(it))


# The live length slot is consulted on every hint, so its failures follow the
# same rules as any other `__length_hint__` failure: a TypeError is swallowed
# in favour of the default, anything else propagates.
class BadLenSeq:
    def __len__(self):
        raise TypeError("nope")

    def __getitem__(self, i):
        if i >= 3:
            raise IndexError
        return i


class ValueErrorLenSeq:
    def __len__(self):
        raise ValueError("nope")

    def __getitem__(self, i):
        if i >= 3:
            raise IndexError
        return i


class NegativeLenSeq:
    def __len__(self):
        return -1

    def __getitem__(self, i):
        if i >= 3:
            raise IndexError
        return i


show("badlen-hint", lambda: operator.length_hint(iter(BadLenSeq())))
show("badlen-hint-default", lambda: operator.length_hint(iter(BadLenSeq()), 6))
show("badlen-direct", lambda: iter(BadLenSeq()).__length_hint__())
show("valueerrorlen-hint", lambda: operator.length_hint(iter(ValueErrorLenSeq())))
show("negativelen-hint", lambda: operator.length_hint(iter(NegativeLenSeq())))


# `reversed()` captures its starting index once, then re-reads the length.
class FlakySeq:
    broken = False

    def __len__(self):
        if FlakySeq.broken:
            raise TypeError("later")
        return 3

    def __getitem__(self, i):
        if i >= 3:
            raise IndexError
        return i


it = reversed(FlakySeq())
FlakySeq.broken = True
show("flaky-rev-hint", lambda: operator.length_hint(it))
show("flaky-rev-direct", lambda: it.__length_hint__())
show("flaky-rev-list", lambda: list(it))
FlakySeq.broken = False

print("--- the len -> __length_hint__ -> default chain ---")


class NoLen:
    pass


class HasHint:
    def __length_hint__(self):
        return 7


class BoolHint:
    def __length_hint__(self):
        return True


class NegHint:
    def __length_hint__(self):
        return -1


class FloatHint:
    def __length_hint__(self):
        return 1.5


class IndexHint:
    class Idx:
        def __index__(self):
            return 4

    def __length_hint__(self):
        return IndexHint.Idx()


class NotImplHint:
    def __length_hint__(self):
        return NotImplemented


class TypeErrorHint:
    def __length_hint__(self):
        raise TypeError("boom")


class ValueErrorHint:
    def __length_hint__(self):
        raise ValueError("boom")


class LenWins:
    def __len__(self):
        return 2

    def __length_hint__(self):
        return 9


class LenTypeError:
    def __len__(self):
        raise TypeError("len boom")

    def __length_hint__(self):
        return 5


class LenValueError:
    def __len__(self):
        raise ValueError("len boom")


class LenNegative:
    def __len__(self):
        return -1


show("no-slot", lambda: operator.length_hint(NoLen()))
show("no-slot-default", lambda: operator.length_hint(NoLen(), 5))
show("hint", lambda: operator.length_hint(HasHint()))
show("hint-bool", lambda: operator.length_hint(BoolHint()))
show("hint-negative", lambda: operator.length_hint(NegHint()))
show("hint-float", lambda: operator.length_hint(FloatHint()))
show("hint-index-object", lambda: operator.length_hint(IndexHint()))
show("hint-notimplemented", lambda: operator.length_hint(NotImplHint()))
show("hint-notimplemented-default", lambda: operator.length_hint(NotImplHint(), 3))
show("hint-typeerror", lambda: operator.length_hint(TypeErrorHint()))
show("hint-typeerror-default", lambda: operator.length_hint(TypeErrorHint(), 3))
show("hint-valueerror", lambda: operator.length_hint(ValueErrorHint()))
show("len-wins", lambda: operator.length_hint(LenWins()))
show("len-typeerror-falls-through", lambda: operator.length_hint(LenTypeError()))
show("len-valueerror-propagates", lambda: operator.length_hint(LenValueError()))
show("len-negative", lambda: operator.length_hint(LenNegative()))

# The lookup is on the type, so an instance attribute is ignored.
inst = NoLen()
inst.__length_hint__ = lambda: 99
show("instance-attribute-ignored", lambda: operator.length_hint(inst))


class Meta(type):
    def __length_hint__(cls):
        return 11


class WithMeta(metaclass=Meta):
    pass


show("metaclass-slot", lambda: operator.length_hint(WithMeta))

print("--- signature ---")
show("default-negative", lambda: operator.length_hint(NoLen(), -3))
show("default-bool", lambda: operator.length_hint(NoLen(), True))
show("default-index-object", lambda: operator.length_hint(NoLen(), IndexHint.Idx()))
show("default-float", lambda: operator.length_hint(NoLen(), 1.0))
show("default-str", lambda: operator.length_hint(NoLen(), "x"))
show("default-none", lambda: operator.length_hint(NoLen(), None))
show("no-args", lambda: operator.length_hint())
show("three-args", lambda: operator.length_hint([], 1, 2))
show("keyword-default", lambda: operator.length_hint(NoLen(), default=4))
show("keyword-obj", lambda: operator.length_hint(obj=[]))
show("name", lambda: operator.length_hint.__name__)

print("--- huge counts narrow to Py_ssize_t ---")


class HugeHint:
    def __length_hint__(self):
        return 2**70


class HugeNegHint:
    def __length_hint__(self):
        return -(2**70)


show("huge-hint", lambda: operator.length_hint(HugeHint()))
show("huge-negative-hint", lambda: operator.length_hint(HugeNegHint()))
show("huge-default", lambda: operator.length_hint(NoLen(), 2**70))
show("wide-range-direct", lambda: iter(range(10**30)).__length_hint__())
show("wide-range-hint", lambda: operator.length_hint(iter(range(10**30))))
show("big-range-hint", lambda: operator.length_hint(iter(range(10**18))))

print("--- list()/tuple() still materialise the same elements ---")
it = iter([1, 2, 3, 4])
next(it)
print(list(it), operator.length_hint(it))
it = reversed([1, 2, 3, 4])
next(it)
print(list(it), operator.length_hint(it))
print(list(iter(range(4))), tuple(iter("abc")), list(reversed(Seq())))
