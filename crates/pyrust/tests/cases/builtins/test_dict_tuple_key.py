# Parity fixture for issue #918:
# dict subscript with a tuple key must work just like d.get((1,2,3)).

# 3-element tuple key — previously incorrectly treated as a slice descriptor.
d = {(1, 2, 3): "x"}
print(d[(1, 2, 3)])
print(d[1, 2, 3])          # comma-separated form is syntactic sugar for the same tuple
print(d.get((1, 2, 3)))

# single-element tuple key
d2 = {(1,): "a", (2,): "b"}
print(d2[(1,)])
print(d2[(2,)])
print(d2[1,])

# 2-element tuple key
d3 = {(1, 2): "c"}
print(d3[(1, 2)])

# 4-element tuple key
d4 = {(1, 2, 3, 4): "d"}
print(d4[(1, 2, 3, 4)])

# tuple key via SetItem
d5 = {}
d5[(1, 2, 3)] = "assigned"
print(d5[(1, 2, 3)])

# tuple key via DeleteItem
d6 = {(1, 2, 3): "x"}
del d6[(1, 2, 3)]
print(len(d6))

# KeyError for missing tuple key
try:
    _ = d[(9, 9, 9)]
except KeyError as e:
    print(f"KeyError: {e}")

# Slicing a list still works correctly
lst = [10, 20, 30, 40, 50]
print(lst[1:3])
print(lst[::2])
