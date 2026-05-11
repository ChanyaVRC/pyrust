# enumerate, zip, reversed, sorted

# --- enumerate ---

# basic index-value pairs
for i, v in enumerate(['a', 'b', 'c']):
    print("enum", i, v)

# enumerate with start offset
for i, v in enumerate(['x', 'y', 'z'], 10):
    print("enum-start", i, v)

# enumerate over a range
for i, v in enumerate(range(3)):
    print("enum-range", i, v)

# list(enumerate(...)) repr
print("enum-list", list(enumerate(['p', 'q'])))

# --- zip ---

# two equal-length lists
for a, b in zip([1, 2, 3], ['one', 'two', 'three']):
    print("zip-pair", a, b)

# truncates to shortest
result = list(zip([1, 2, 3, 4], ['a', 'b']))
print("zip-short", result)

# three iterables
for a, b, c in zip([1, 2], [3, 4], [5, 6]):
    print("zip-triple", a, b, c)

# empty zip
print("zip-empty", list(zip([], [1, 2])))

# --- reversed ---

# list
print("rev-list", list(reversed([1, 2, 3, 4, 5])))

# range
print("rev-range", list(reversed(range(5))))

# string characters
print("rev-str", list(reversed('abc')))

# --- sorted ---

# integers ascending
print("sort-int", sorted([3, 1, 4, 1, 5, 9, 2, 6]))

# descending
print("sort-desc", sorted([3, 1, 4, 1, 5], reverse=True))

# strings
print("sort-str", sorted(['banana', 'apple', 'cherry', 'date']))

# key=len
print("sort-key", sorted(['bb', 'aaa', 'c'], key=len))

# already sorted
print("sort-sorted", sorted([1, 2, 3]))

# single element
print("sort-one", sorted([42]))

# stable sort: equal-key items preserve order
pairs = [(2, 'b'), (1, 'c'), (2, 'a'), (1, 'd')]
print("sort-stable", sorted(pairs, key=lambda p: p[0]))
