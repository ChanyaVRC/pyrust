# Parity test for slice hashing with unhashable components (issue #850).
#
# CPython 3.12 made slice objects hashable when all components are hashable.
# When a component is unhashable, the TypeError should name the *component*
# type ("unhashable type: 'list'"), not the slice type.

# Unhashable start
try:
    hash(slice([1, 2], 3))
    print("FAIL: expected TypeError")
except TypeError as e:
    print("start:", e)

# Unhashable stop
try:
    hash(slice(1, [3, 4]))
    print("FAIL: expected TypeError")
except TypeError as e:
    print("stop:", e)

# Unhashable step
try:
    hash(slice(None, None, [1]))
    print("FAIL: expected TypeError")
except TypeError as e:
    print("step:", e)

# Unhashable set (another unhashable type)
try:
    hash(slice({1, 2}, 5))
    print("FAIL: expected TypeError")
except TypeError as e:
    print("set bound:", e)

# Unhashable dict
try:
    hash(slice({"a": 1}, 5))
    print("FAIL: expected TypeError")
except TypeError as e:
    print("dict bound:", e)

# Slices with all-hashable components are usable as dict keys and in sets.
d = {slice(1, 2): "a", slice(None, 5, 2): "b"}
print("dict slice(1,2):", d[slice(1, 2)])
print("dict slice(None,5,2):", d[slice(None, 5, 2)])

s = {slice(0, 10), slice(0, 10, 2)}
print("set size:", len(s))
print("in set:", slice(0, 10) in s)

# Equal slices have equal hashes.
s1 = slice(1, 2, 3)
s2 = slice(1, 2, 3)
print("equal hash:", hash(s1) == hash(s2))

# Unhashable slice as dict key
try:
    d2 = {slice([1], 2): "x"}
    print("FAIL: expected TypeError")
except TypeError as e:
    print("dict key unhashable:", e)

# Unhashable slice in set literal
try:
    s2 = {slice([1], 2)}
    print("FAIL: expected TypeError")
except TypeError as e:
    print("set unhashable:", e)

# Tuples containing hashable slices are hashable.
t1 = (slice(1, 2),)
t2 = (slice(1, 2),)
print("tuple+slice hashable:", hash(t1) == hash(t2))
print("tuple+two slices hashable:", hash((slice(1, 2), slice(3, 4))) == hash((slice(1, 2), slice(3, 4))))

# Tuple containing a slice with an unhashable bound raises TypeError naming
# the bound type, not 'slice' or 'tuple'.
try:
    hash((slice([1], 2),))
    print("FAIL: expected TypeError")
except TypeError as e:
    print("tuple+slice+list bound:", e)

try:
    hash((slice(1, {2, 3}),))
    print("FAIL: expected TypeError")
except TypeError as e:
    print("tuple+slice+set bound:", e)

print("done")
