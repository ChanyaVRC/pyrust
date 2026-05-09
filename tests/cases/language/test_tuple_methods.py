t = (3, 1, 4, 1, 5)

# index
assert t.index(4) == 2
assert t.index(1) == 1
assert t.index(1, 2) == 3

# count
assert t.count(1) == 2
assert t.count(9) == 0

print("tuple methods OK")
