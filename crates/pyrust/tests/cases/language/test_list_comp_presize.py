# List comprehensions with a single unconditional clause pre-size their result
# list from the source length (Insn::BuildListReserve). The reservation is a
# capacity hint only, so every case here must match CPython byte-for-byte.

# --- presize-eligible sources (single clause, no `if`) ---
print([x * 2 for x in range(20)])
print([x for x in []])
print([x for x in [10, 20, 30]])
print([x for x in (1, 2, 3)])
print([c for c in "héllo😀"])  # str iterates by scalar; multibyte-safe
print([b for b in b"abc"])  # bytes -> ints
print([x for x in {1, 2, 3, 4, 5}])  # set (order-independent count)
print([k for k in {"a": 1, "b": 2, "c": 3}])  # dict keys
print([x for x in range(0)])  # empty range
print([x for x in range(1)])  # single-element range
print([x for x in range(10, 0, -2)])  # negative step
print([x for x in bytearray(b"xyz")])  # unknown-len source -> reserve nothing
print(sorted([x for x in frozenset([3, 1, 2])]))

# --- NOT presize-eligible: element count != source length ---
print([x for x in range(20) if x % 2 == 0])  # condition
print([x * y for x in range(3) for y in range(3)])  # nested for


def g():
    yield 1
    yield 2
    yield 3


print([x for x in g()])  # generator: unknown length


# lazy consumption + side-effect ordering must be unchanged
class It:
    def __init__(self):
        self.n = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n > 3:
            raise StopIteration
        return self.n


print([x for x in It()])

# result must be a genuine, independent, mutable list
a = [x for x in range(3)]
b = [x for x in range(3)]
a.append(99)
print(a, b)
print([x for x in range(2)] is [x for x in range(2)])

# walrus target still leaks to the enclosing scope
print([y := x for x in range(3)], y)

# comprehension inside a function
def sq():
    return [i * i for i in range(5)]


print(sq())

# larger source to exercise growth past the pre-reserved capacity boundaries
print(len([x for x in range(1000)]))
print(sum([x for x in range(100)]))
