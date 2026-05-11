a = [3, 1, 4, 1, 5, 9, 2, 6]

# append
a.append(7)
assert a[-1] == 7

# pop (no arg → last)
v = a.pop()
assert v == 7

# pop with index
v = a.pop(0)
assert v == 3

# insert
a.insert(0, 0)
assert a[0] == 0

# remove
a.remove(0)
assert a[0] == 1

# extend
a.extend([10, 11])
assert a[-1] == 11

# count
assert a.count(1) == 2

# index
assert a.index(9) == 4

# index with start
assert a.index(1, 1) == 2

# copy
b = a.copy()
b.append(99)
assert 99 not in a

# reverse
c = [1, 2, 3]
c.reverse()
assert c == [3, 2, 1]

# sort ascending
d = [3, 1, 4, 1, 5, 9]
d.sort()
assert d == [1, 1, 3, 4, 5, 9]

# sort descending
d.sort(reverse=True)
assert d == [9, 5, 4, 3, 1, 1]

# clear
e = [1, 2, 3]
e.clear()
assert e == []

print("list methods OK")
