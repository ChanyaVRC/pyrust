# Parity fixture: dict.update() error messages match CPython 3.12.
# Non-iterable arguments must propagate 'X object is not iterable';
# iterable-but-wrong-element arguments use CPython's indexed messages.

# Non-iterable: int
try:
    {}.update(42)
except TypeError as e:
    print(e)

# Non-iterable: float
try:
    {}.update(3.14)
except TypeError as e:
    print(e)

# Non-iterable: None
try:
    {}.update(None)
except TypeError as e:
    print(e)

# Augmented assign with non-iterable
d = {}
try:
    d |= 42
except TypeError as e:
    print(e)

# String: iterable but each char has length 1, not 2
try:
    {}.update("hello")
except ValueError as e:
    print(e)

# List of ints: int is not a sequence
try:
    {}.update([1, 2, 3])
except TypeError as e:
    print(e)

# List of wrong-length tuples at index 0
try:
    {}.update([(1,)])
except ValueError as e:
    print(e)

# Wrong-length tuple at index 1 (first pair is valid)
try:
    {}.update([(1, 2), (3,)])
except ValueError as e:
    print(e)

# Non-sequence at index 1 (error at index 1)
try:
    {}.update([(1, 2), 99])
except TypeError as e:
    print(e)

# Happy path: list of pairs
d = {}
d.update([(10, 20), (30, 40)])
print(sorted(d.items()))

# Happy path: dict argument
d2 = {}
d2.update({50: 60})
print(d2[50])

# Happy path: tuple of pairs
d3 = {}
d3.update(((100, 200),))
print(d3[100])
