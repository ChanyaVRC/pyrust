# Parity fixture for issue #1245: map() and filter() lazy semantics.
# Focuses on type identity, next(), exhaustion, and multi-iterable map.

# type() name checks
print(type(map(str, range(3))).__name__)
print(type(filter(None, [0, 1, 2])).__name__)

# next() on map
m = map(str, range(3))
print(next(m))
print(next(m))
print(next(m))
try:
    next(m)
except StopIteration:
    print('StopIteration')

# map exhaustion: second list() call returns []
m2 = map(str, range(2))
list(m2)
print(list(m2))

# multi-iterable map stops at shortest
print(list(map(lambda x, y: x + y, [1, 2], [3, 4])))
print(list(map(lambda x, y: x + y, [1, 2, 3], [10, 20])))
print(list(map(lambda x, y, z: x * y + z, [1, 2], [3, 4], [5, 6])))

# next() on filter (func=None keeps truthy)
f = filter(None, [0, 1, 2])
print(next(f))
print(next(f))
try:
    next(f)
except StopIteration:
    print('StopIteration')

# filter exhaustion
f2 = filter(None, [1, 2])
list(f2)
print(list(f2))

# filter with callable
print(list(filter(lambda x: x % 2 == 0, [1, 2, 3, 4, 5])))

# map with user-defined function
def add_one(x):
    return x + 1

print(list(map(add_one, [10, 20, 30])))

# map with range source
print(list(map(str, range(4))))
