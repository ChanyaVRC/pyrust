# Regression: list.extend should accept any iterable, not just list/tuple/str/range.
# See issue #427.

# tuple
xs = [1, 2]
xs.extend((3, 4))
assert xs == [1, 2, 3, 4]

# range
xs = [1]
xs.extend(range(3))
assert xs == [1, 0, 1, 2]

# str (iterates characters)
xs = []
xs.extend("ab")
assert xs == ["a", "b"]

# set (order-insensitive)
xs = [1, 2]
xs.extend({3, 4})
xs.sort()
assert xs == [1, 2, 3, 4]

# dict (iterates keys)
xs = []
xs.extend({"a": 1})
assert xs == ["a"]

# dict.keys()
xs = []
xs.extend({"k": 1}.keys())
assert xs == ["k"]

# dict.values()
xs = []
xs.extend({"k": 7}.values())
assert xs == [7]

# iter()
xs = [1]
xs.extend(iter([2, 3]))
assert xs == [1, 2, 3]

# reversed()
xs = [1]
xs.extend(reversed([2, 3]))
assert xs == [1, 3, 2]

# self-alias (also a regression check for #414 — must not deadlock)
xs = [1, 2]
xs.extend(xs)
assert xs == [1, 2, 1, 2]

# non-iterable must still raise
threw = False
try:
    xs = [1]
    xs.extend(42)
except TypeError:
    threw = True
assert threw

print("list.extend iterables OK")
