# Issue #3000 follow-up: zip/map/filter/enumerate/reversed are subclassable
# built-in iterator types in CPython.  Their subclasses retain their Python
# class identity while the inherited native constructor owns iterator state.

import copy


class ZipSub(zip):
    pass


class MapSub(map):
    pass


class FilterSub(filter):
    pass


class EnumerateSub(enumerate):
    pass


class ReversedSub(reversed):
    pass


instances = [
    ("zip", ZipSub([1, 2], "ab"), ZipSub, zip),
    ("map", MapSub(str, [1, 2]), MapSub, map),
    ("filter", FilterSub(None, [0, 1, 2]), FilterSub, filter),
    ("enumerate", EnumerateSub("ab", start=5), EnumerateSub, enumerate),
    ("reversed", ReversedSub("ab"), ReversedSub, reversed),
]

# The carrier is the subclass instance itself: iter() preserves identity,
# next() advances its retained built-in backing, and list() sees the remainder.
for label, value, subtype, base in instances:
    print(label, type(value) is subtype, isinstance(value, subtype), isinstance(value, base))
    print(iter(value) is value, next(value), list(value))

# zip/map retain their variadic constructor contracts; zip also retains its
# zero-input and strict keyword forms.
print("zip-empty", list(ZipSub()))
print("zip-many", list(ZipSub([1, 2], "ab", range(2))))
print("map-many", list(MapSub(lambda a, b: a + b, [1, 2], [3, 4])))


# reversed is also a factory: specialised sequence cursors and a user
# __reversed__ result pass through unchanged.  Only its generic native cursor
# is wrapped in the requested subclass.
class GenericReverseSeq:
    def __init__(self):
        self.data = [1, 2, 3]

    def __len__(self):
        return len(self.data)

    def __getitem__(self, index):
        return self.data[index]


class ReverseSub(reversed):
    pass


foreign_reverse = iter((9, 8))
foreign_generic_reverse = reversed((7, 6))


class ForeignReverse:
    def __reversed__(self):
        return foreign_reverse


class ForeignGenericReverse:
    def __reversed__(self):
        return foreign_generic_reverse


print(
    "reversed-results",
    type(ReverseSub([1, 2])) is ReverseSub,
    type(ReverseSub((1, 2))) is ReverseSub,
    type(ReverseSub("ab")) is ReverseSub,
    type(ReverseSub(range(2))) is ReverseSub,
    type(ReverseSub(GenericReverseSeq())) is ReverseSub,
    ReverseSub(ForeignReverse()) is foreign_reverse,
    ReverseSub(ForeignGenericReverse()) is foreign_generic_reverse,
)

# User construction hooks compose with the inherited native allocator.
class InitZip(zip):
    def __init__(self, *args, **kwargs):
        self.seen = (len(args), sorted(kwargs))


init_zip = InitZip([1], [2], strict=True)
print("custom-init", type(init_zip) is InitZip, init_zip.seen, list(init_zip))


class NewMap(map):
    def __new__(cls, *args):
        return super().__new__(cls, *args)


new_map = NewMap(str, [7])
print("custom-new", type(new_map) is NewMap, next(new_map))


def expect_error(label, expected, thunk):
    try:
        thunk()
        print(label, False)
    except Exception as exc:
        print(label, type(exc) is expected)


# Required/excess argument and keyword validation remains owned by each
# existing registry constructor, including zip(strict=True)'s deferred error.
expect_error("zip-keyword", TypeError, lambda: ZipSub(nope=True))
expect_error("zip-strict", ValueError, lambda: list(ZipSub([1], [2, 3], strict=True)))
expect_error("map-empty", TypeError, lambda: MapSub())
expect_error("map-one", TypeError, lambda: MapSub(str))
expect_error("map-keyword", TypeError, lambda: MapSub(str, [1], nope=True))
expect_error("filter-one", TypeError, lambda: FilterSub(None))
expect_error("filter-three", TypeError, lambda: FilterSub(None, [1], [2]))
expect_error("enumerate-empty", TypeError, lambda: EnumerateSub())
expect_error("enumerate-keyword", TypeError, lambda: EnumerateSub([1], nope=True))
expect_error("reversed-empty", TypeError, lambda: ReversedSub())
expect_error("reversed-two", TypeError, lambda: ReversedSub([1], [2]))

# Unbound native slots validate the defining type, and slice stays the one
# non-subclassable member of the six-class migration.
expect_error("zip-new-type", TypeError, lambda: zip.__new__(int))
expect_error("zip-new-keyword-self", TypeError, lambda: zip.__new__(cls=zip))
expect_error("map-next-type", TypeError, lambda: map.__next__(zip()))
expect_error("map-next-keyword-self", TypeError, lambda: map.__next__(self=map(str, [])))
expect_error("layout-zip-map", TypeError, lambda: type("Mixed", (zip, map), {}))
expect_error("layout-zip-list", TypeError, lambda: type("Mixed", (zip, list), {}))


class ZipObject(zip, object):
    pass


print("layout-compatible", issubclass(ZipObject, zip))


class PlainObjectNew:
    pass


SameNamedZip = type("zip", (), {})


class ObjectNewMixin:
    pass


class MixedZip(ObjectNewMixin, zip):
    pass


# The unsafe-allocation guard follows typed built-in ancestry: ordinary and
# same-named classes remain safe, while a native iterator reached through an
# extra multiple-inheritance base is still rejected.
plain_object = object.__new__(PlainObjectNew)
same_named_zip = object.__new__(SameNamedZip)
print(
    "safe-object-new",
    type(plain_object) is PlainObjectNew,
    type(same_named_zip) is SameNamedZip,
)
expect_error(
    "unsafe-object-new-mixin-zip",
    TypeError,
    lambda: object.__new__(MixedZip),
)
expect_error("unsafe-object-new-slice", TypeError, lambda: object.__new__(slice))

for label, subtype in (
    ("zip", ZipSub),
    ("map", MapSub),
    ("filter", FilterSub),
    ("enumerate", EnumerateSub),
    ("reversed", ReversedSub),
):
    expect_error(
        "unsafe-object-new-" + label,
        TypeError,
        lambda subtype=subtype: object.__new__(subtype),
    )


native_slice = slice.__new__(slice, 1, 4, 2)
print(
    "slice-new",
    "__new__" in slice.__dict__,
    native_slice.start,
    native_slice.stop,
    native_slice.step,
)
expect_error("slice-new-type", TypeError, lambda: slice.__new__(int, 1))
try:
    class SliceSub(slice):
        pass
    print("slice-subclass", False)
except TypeError:
    print("slice-subclass", True)


# copy.copy keeps the subclass carrier but follows each native iterator's
# reduction semantics: zip/map/filter share their shallow inner cursor;
# enumerate shares the source while retaining an independent count; generic
# reversed has an independent index.  deepcopy detaches every retained source.
copy_cases = (
    ("zip", ZipSub([1, 2, 3], "abc")),
    ("map", MapSub(str, [1, 2, 3])),
    ("filter", FilterSub(None, [1, 2, 3, 4])),
    ("enumerate", EnumerateSub("abcd", 4)),
    ("reversed", ReversedSub(GenericReverseSeq())),
)
for label, value in copy_cases:
    first = next(value)
    shallow = copy.copy(value)
    deep = copy.deepcopy(value)
    print("copy-types", label, type(shallow) is type(value), type(deep) is type(value))
    print("copy-cursors", label, first, next(shallow), next(value), next(deep))
