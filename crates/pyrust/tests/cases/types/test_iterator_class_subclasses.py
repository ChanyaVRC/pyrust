# Issue #3000 follow-up: zip/map/filter/enumerate/reversed are subclassable
# built-in iterator types in CPython.  Their subclasses retain their Python
# class identity while the inherited native constructor owns iterator state.


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
try:
    class SliceSub(slice):
        pass
    print("slice-subclass", False)
except TypeError:
    print("slice-subclass", True)
