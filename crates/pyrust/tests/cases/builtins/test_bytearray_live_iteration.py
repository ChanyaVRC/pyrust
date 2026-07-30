# bytearray iteration walks the live buffer by index, re-reading its size on
# every step, so mid-walk growth is yielded and a shrink past the cursor ends
# the walk.  `reversed()` keeps its own rule: a fixed descending index that
# stops as soon as the buffer is shorter than the index it is about to read.


def walk(data, mutate, label):
    ba = bytearray(data)
    out = []
    for value in ba:
        out.append(value)
        if len(out) == 1:
            mutate(ba)
    print(label, out, bytes(ba))


walk(b"ab", lambda ba: ba.append(99), "append:")
walk(b"ab", lambda ba: ba.extend(b"cd"), "extend:")
walk(b"abcdef", lambda ba: ba.__setitem__(slice(4, None), b""), "shrink below cursor:")
walk(b"abcdef", lambda ba: ba.__setitem__(slice(1, None), b""), "shrink past cursor:")
walk(b"abcdef", lambda ba: ba.clear(), "clear:")
walk(b"abcdef", lambda ba: ba.__setitem__(slice(0, 2), b""), "delete front:")
walk(b"abc", lambda ba: ba.__setitem__(slice(1, 2), b"XYZ"), "slice assign grow:")
walk(b"abcdef", lambda ba: ba.__setitem__(slice(1, 5), b"X"), "slice assign shrink:")
walk(b"abcd", lambda ba: ba.__setitem__(1, 90), "replace ahead:")
walk(b"abc", lambda ba: ba.insert(0, 90), "insert front:")
walk(b"abcd", lambda ba: ba.pop(), "pop:")


# Growth is bounded here so the fixture terminates; CPython would loop forever
# if every step appended.
ba = bytearray(b"a")
grown = []
for value in ba:
    grown.append(value)
    if len(grown) < 5:
        ba.append(value + 1)
print("append each:", grown)

ba = bytearray(b"ab")
doubled = []
for value in ba:
    doubled.append(value)
    if len(doubled) == 1:
        ba *= 3
print("in-place repeat:", doubled)

ba = bytearray(b"abcd")
emptied = []
for value in ba:
    emptied.append(value)
    if len(emptied) == 1:
        ba *= 0
print("in-place repeat zero:", emptied)

ba = bytearray(b"abc")
refilled = []
for value in ba:
    refilled.append(value)
    if len(refilled) == 1:
        ba.clear()
        ba.extend(b"XYZ")
print("clear then refill:", refilled)


# An explicit iterator shares the same live cursor.
ba = bytearray(b"a")
iterator = iter(ba)
print("explicit first:", next(iterator))
ba.append(98)
print("explicit after append:", next(iterator))
for label in ("initial", "after append"):
    if label == "after append":
        ba.append(99)
    try:
        next(iterator)
        print("explicit", label, "RESURRECTED")
    except StopIteration:
        # Exhaustion is permanent: CPython drops the buffer reference on the
        # first StopIteration, so a later append cannot revive the iterator.
        print("explicit", label, "stopped")

ba = bytearray()
iterator = iter(ba)
ba.append(7)
print("iter before first append:", list(iterator))

ba = bytearray(b"abc")
iterator = iter(ba)
print("keeps source alive:", next(iterator), end=" ")
del ba
print(list(iterator))

ba = bytearray(b"\x01\x02")
iterator = iter(ba)
ba.append(3)
print("native drain of a live iterator:", sum(iterator))


# `__length_hint__` subtracts the position from the buffer's current size.
ba = bytearray(b"abc")
iterator = iter(ba)
hints = [iterator.__length_hint__()]
next(iterator)
hints.append(iterator.__length_hint__())
ba.append(100)
hints.append(iterator.__length_hint__())
ba.clear()
hints.append(iterator.__length_hint__())
ba.extend(b"zz")
hints.append(iterator.__length_hint__())
print("length hint:", hints)

ba = bytearray(b"a")
iterator = iter(ba)
next(iterator)
try:
    next(iterator)
except StopIteration:
    pass
ba.extend(b"xyz")
print("length hint after exhaustion:", iterator.__length_hint__())


# enumerate shares the element cursor, so its walk is live too.
ba = bytearray(b"ab")
pairs = []
for index, value in enumerate(ba):
    pairs.append((index, value))
    if len(pairs) == 1:
        ba.append(99)
print("enumerate append:", pairs)

ba = bytearray(b"abcdef")
pairs = []
for index, value in enumerate(ba):
    pairs.append((index, value))
    if len(pairs) == 2:
        del ba[3:]
print("enumerate shrink:", pairs)

ba = bytearray(b"abcd")
iterator = iter(ba)
counted = enumerate(iterator)
interleaved = [next(counted)]
ba.append(99)
interleaved.append(next(iterator))
interleaved.append(next(counted))
print("enumerate aliased with its inner iterator:", interleaved)


# reversed() reads live elements from a descending index it never re-anchors.
ba = bytearray(b"ab")
backward = []
for value in reversed(ba):
    backward.append(value)
    if len(backward) == 1:
        ba.append(99)
print("reversed append:", backward)

ba = bytearray(b"abcdef")
backward = []
for value in reversed(ba):
    backward.append(value)
    if len(backward) == 1:
        del ba[3:]
print("reversed shrink:", backward)

ba = bytearray(b"abcd")
backward = []
for value in reversed(ba):
    backward.append(value)
    if len(backward) == 1:
        ba[0] = 90
print("reversed replace ahead:", backward)

ba = bytearray(b"abc")
iterator = reversed(ba)
hints = [iterator.__length_hint__()]
next(iterator)
hints.append(iterator.__length_hint__())
ba.append(100)
hints.append(iterator.__length_hint__())
ba.clear()
hints.append(iterator.__length_hint__())
print("reversed length hint:", hints)

ba = bytearray()
iterator = reversed(ba)
ba.append(7)
print("reversed before first append:", list(iterator))


# Adapters wrapping the bytearray see the same live elements.
ba = bytearray(b"ab")
mapped = map(lambda value: value, ba)
collected = [next(mapped)]
ba.append(99)
collected.extend(mapped)
print("map append:", collected)

ba = bytearray(b"ab")
zipped = zip(ba, "xyz")
collected = [next(zipped)]
ba.append(99)
collected.extend(zipped)
print("zip append:", collected)

ba = bytearray(b"ab")
generated = (value for value in ba)
collected = [next(generated)]
ba.append(99)
collected.extend(generated)
print("genexp append:", collected)


# Subclasses inherit the same live iterator slot.
class ByteArraySubclass(bytearray):
    pass


ba = ByteArraySubclass(b"ab")
out = []
for value in ba:
    out.append(value)
    if len(out) == 1:
        ba.append(99)
print("subclass append:", out)

ba = ByteArraySubclass(b"abc")
iterator = iter(ba)
next(iterator)
ba.append(9)
print("subclass length hint:", iterator.__length_hint__())
print("subclass reversed:", list(reversed(ByteArraySubclass(b"abc"))))


class OverriddenIter(bytearray):
    def __iter__(self):
        return iter([1, 2])


print("subclass __iter__ override:", list(OverriddenIter(b"abcd")))


# Empty and single-element buffers, and the whole-object consumers that drain
# without running Python code in between.
print("empty:", list(bytearray()), list(iter(bytearray())), list(reversed(bytearray())))
print("single:", list(bytearray(b"\x00")))
values = bytearray(b"\x01\x02\x03")
print("consumers:", list(values), tuple(values), sum(values), bytes(values))
print("no snapshot for a large buffer:", sum(1 for _ in bytearray(100000)))
