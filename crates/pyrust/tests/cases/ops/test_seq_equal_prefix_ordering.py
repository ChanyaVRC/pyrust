# Issue #2216: tuple/list ordering scans the `==`-equal prefix and only
# applies the ordering op to the first *differing* element, so an equal
# prefix containing unorderable values (e.g. None) does not raise.

# Equal None-prefix: compares equal, no TypeError.
print((1, 2, None) <= (1, 2, None))
print((1, 2, None) >= (1, 2, None))
print([1, None, 3] < [1, None, 4])
print([1, None, 4] > [1, None, 3])
print([None, None] == [None, None])

# First differing element decides ordering when the prefix is equal.
print((1, None) < (2, None))
print((1, None, 3) < (1, None, 3, 4))   # equal prefix, shorter is smaller
print((1, 2) < (1, 2, 3))

# Nested sequences compare element-wise too.
print(((1, None), 3) < ((1, None), 4))
print([[1, None], [2]] < [[1, None], [3]])

# The first *differing* pair being unorderable still raises (None vs int).
for a, b in [((1, None), (1, 2)), ([None], [1])]:
    try:
        a < b
    except TypeError as e:
        print("TypeError", e)

# Non-None ordering is unregressed.
print((1, 2, 3) < (1, 2, 4))
print(["a", "b"] < ["a", "c"])
print((3, 1) > (2, 9))

# A NaN prefix element is `!=` itself but orders Equal, so the same `x is y`
# object skips past it (CPython's Py_EQ identity shortcut) and the next
# element decides — it must not short-circuit to the NaN's Equal ordering.
nan = float("nan")
print((nan, 1) < (nan, 2))
print((nan, 2) < (nan, 1))
print([nan, "a"] < [nan, "b"])
