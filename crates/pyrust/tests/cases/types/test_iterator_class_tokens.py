# Issue #3000: zip / map / filter / enumerate / slice / reversed were modelled
# as `BuiltinFunction(name)` class-tokens rather than real classes, so
# `type(zip(...))` reprd as `<built-in function zip>`, `issubclass(type(z),
# object)` was False and `type(z).__mro__` raised AttributeError.  They are now
# real per-thread PyClass singletons, like `range` (#1793) and the primitives
# (#463).

builtin_types = [zip, map, filter, enumerate, slice, reversed, range]

# The type objects themselves are instances of `type`.
for t in builtin_types:
    print(type(t) is type, isinstance(t, type))

# repr / __name__ / __qualname__ / __module__ match CPython.
for t in builtin_types:
    print(repr(t), "|", t.__name__, "|", t.__qualname__, "|", t.__module__)

# Their MRO terminates at the shared `object` singleton.
for t in builtin_types:
    print(t.__mro__ == (t, object), t.__bases__ == (object,))

instances = [
    ("zip", zip([1], [2])),
    ("map", map(str, [1])),
    ("filter", filter(None, [1])),
    ("enumerate", enumerate([1])),
    ("slice", slice(1, 2)),
]

# `type(instance)` is a real class: proper repr, issubclass of object, and a
# readable __mro__ / __name__ / __module__.
for label, value in instances:
    t = type(value)
    print(label, "|", repr(t), "|", issubclass(t, object), "|", t.__mro__)
    print(label, "|", t.__name__, "|", t.__qualname__, "|", t.__module__)

# Regression guard: identity and isinstance already worked and must keep working.
z = zip([1], [2])
print(isinstance(z, zip), type(z) is zip)
m = map(str, [1])
print(isinstance(m, map), type(m) is map)
f = filter(None, [1])
print(isinstance(f, filter), type(f) is filter)
e = enumerate([1])
print(isinstance(e, enumerate), type(e) is enumerate)
s = slice(1, 2)
print(isinstance(s, slice), type(s) is slice)
r = range(3)
print(isinstance(r, range), type(r) is range)

# The backing-kind fast path distinguishes the six classes exactly; it must
# not turn a different iterator class (or an unrelated primitive) into a hit.
print(isinstance(z, map), isinstance(m, zip), isinstance(s, enumerate))
print(isinstance(1, zip), isinstance(z, int))

# The classes are usable as `issubclass` arguments in both directions.
print(issubclass(zip, object), issubclass(object, zip))
print(issubclass(slice, object), issubclass(slice, slice))

# `reversed(x)` returns a type-specific cursor, exactly as in CPython: `list`
# and `range` have their own reverse-iterator types, while every other sequence
# gets the generic cursor whose type *is* the `reversed` class.
for label, value in [
    ("list", reversed([1, 2])),
    ("str", reversed("ab")),
    ("tuple", reversed((1, 2))),
    ("range", reversed(range(3))),
]:
    print(label, "|", type(value).__name__)

generic = reversed("ab")
print(type(generic) is reversed, isinstance(generic, reversed))
print(type(reversed((1, 2))) is reversed, issubclass(type(generic), object))
print(type(generic).__mro__ == (reversed, object))
# A list has its own reverse iterator, so it is *not* an instance of `reversed`.
print(type(reversed([1, 2])) is reversed, isinstance(reversed([1, 2]), reversed))


class Seq:
    def __getitem__(self, index):
        return [10, 20][index]

    def __len__(self):
        return 2


# The __getitem__/__len__ fallback also yields the generic `reversed` cursor.
print(type(reversed(Seq())) is reversed, list(reversed(Seq())))

# The values still iterate correctly after the migration.
print(list(zip([1, 2], "ab")))
print(list(map(str, [1, 2])))
print(list(filter(None, [0, 1, 2])))
print(list(enumerate("ab")))
print(list(reversed([1, 2, 3])))
print([1, 2, 3, 4][slice(1, 3)])
print(slice(1, 5, 2).start, slice(1, 5, 2).stop, slice(1, 5, 2).step)
print(repr(slice(1, 5, 2)))

# Constructor errors still report the class name CPython uses.
try:
    slice()
except TypeError as exc:
    print("TypeError:", exc)
try:
    enumerate()
except TypeError as exc:
    print("enumerate() with no arguments raises TypeError")

# CPython refuses `class S(slice)` and `class R(range)`; both are non-heap
# types without the Py_TPFLAGS_BASETYPE flag.
for t in (slice, range):
    try:
        class Sub(t):
            pass
        print(t.__name__, "subclassed")
    except TypeError as exc:
        print("TypeError:", exc)
