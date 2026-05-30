# dict.update() with non-primitive iterables (issue #1751)
# CPython 3.12 parity: range, generators, itertools-style objects all work
# via the sequence-of-pairs protocol, raising element-level errors.

# --- error cases ---

# range produces integers; integers are not sequences
try:
    {}.update(range(3))
except TypeError as e:
    print(e)

# a generator of non-sequences (integers) must raise the same error
try:
    {}.update(x for x in range(3))
except TypeError as e:
    print(e)

# wrong-length element: ValueError, not TypeError
try:
    {}.update([(1, 2, 3)])
except ValueError as e:
    print(e)

# wrong-length from a generator
try:
    {}.update((1,) for _ in range(1))
except ValueError as e:
    print(e)

# --- success cases ---

# generator of 2-tuples
d = {}
d.update((x, x * 2) for x in range(3))
print(sorted(d.items()))

# iter() over a list of tuples
d = {}
d.update(iter([(1, 2), (3, 4)]))
print(sorted(d.items()))

# range with only kwargs (no positional) — must still work
d = {}
d.update(a=1, b=2)
print(sorted(d.items()))

# generator expression — kwarg after it
d = {}
d.update(((k, v) for k, v in [("x", 10)]), y=20)
print(sorted(d.items()))

# existing primitive-arg paths are unaffected
d = {}
d.update({"a": 1, "b": 2})
print(sorted(d.items()))

d = {}
d.update([("c", 3), ("d", 4)])
print(sorted(d.items()))

d = {}
d.update((("e", 5), ("f", 6)))
print(sorted(d.items()))
