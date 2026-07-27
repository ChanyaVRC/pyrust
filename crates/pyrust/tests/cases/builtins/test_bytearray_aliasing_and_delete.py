# bytearray mutation must snapshot an aliased right-hand side before taking its
# write lock, and extended slice deletion must preserve CPython's index rules.

same = bytearray(b"abcd")
same[:] = same
print(same)

inserted = bytearray(b"abcd")
inserted[1:1] = inserted
print(inserted)

reversed_self = bytearray(b"abcd")
reversed_self[::-1] = reversed_self
print(reversed_self)

extended = bytearray(b"abcd")
extended.extend(extended)
print(extended)

# VM-backed iterables are fully consumed before the bytearray storage layer
# takes its mutable borrow.
generated = bytearray(b"x")
generated[:] = (value for value in (65, 66))
print(generated)


class Integer:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        print("index", self.value)
        return self.value


class Values:
    def __iter__(self):
        print("iter")
        return iter((Integer(67), Integer(68)))


generated[1:1] = Values()
print(generated)

positive = bytearray(range(100))
del positive[1::3]
print(len(positive), positive[:6], positive[-6:])

negative = bytearray(range(100))
del negative[97:2:-4]
print(len(negative), negative[:6], negative[-6:])

# bool is an integer index. `True` is out of range for a one-byte receiver and
# must raise IndexError rather than reaching Vec::remove(1).
try:
    bytearray(b"x").pop(True)
except Exception as exc:
    print(type(exc).__name__, str(exc))

# This expression stays in the native signed-integer range. Negating it again
# inside index normalization must not overflow.
minimum_i64 = -9223372036854775807 - 1
for operation in (
    lambda: bytearray(b"x")[minimum_i64],
    lambda: bytearray(b"x").pop(minimum_i64),
):
    try:
        operation()
    except Exception as exc:
        print(type(exc).__name__, str(exc))

front = bytearray(b"bc")
front.insert(minimum_i64, ord("a"))
print(front)
