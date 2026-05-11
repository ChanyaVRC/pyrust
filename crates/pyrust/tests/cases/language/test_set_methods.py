s = {1, 2, 3}

# add
s.add(4)
assert 4 in s

# discard (present)
s.discard(4)
assert 4 not in s

# discard (absent — no error)
s.discard(99)

# remove (present)
s.add(10)
s.remove(10)
assert 10 not in s

# remove (absent — KeyError)
try:
    s.remove(99)
    assert False, "should have raised KeyError"
except KeyError:
    pass

# pop
s2 = {42}
v = s2.pop()
assert v == 42
assert len(s2) == 0

# pop from empty set — KeyError
try:
    s2.pop()
    assert False, "should have raised KeyError"
except KeyError:
    pass

# clear
s3 = {1, 2, 3}
s3.clear()
assert s3 == set()

# copy
s4 = {1, 2, 3}
s5 = s4.copy()
s5.add(99)
assert 99 not in s4

# update
s6 = {1, 2}
s6.update([3, 4], {5})
assert s6 == {1, 2, 3, 4, 5}

# union
u = {1, 2}.union({2, 3}, {4})
assert u == {1, 2, 3, 4}

# intersection
i = {1, 2, 3}.intersection({2, 3, 4})
assert i == {2, 3}

# intersection_update
s7 = {1, 2, 3}
s7.intersection_update({2, 3, 4})
assert s7 == {2, 3}

# difference
d = {1, 2, 3}.difference({2})
assert d == {1, 3}

# difference_update
s8 = {1, 2, 3}
s8.difference_update({2, 3})
assert s8 == {1}

# symmetric_difference
sd = {1, 2, 3}.symmetric_difference({2, 3, 4})
assert sd == {1, 4}

# symmetric_difference_update
s9 = {1, 2, 3}
s9.symmetric_difference_update({2, 3, 4})
assert s9 == {1, 4}

# issubset
assert {1, 2}.issubset({1, 2, 3})
assert not {1, 4}.issubset({1, 2, 3})

# issuperset
assert {1, 2, 3}.issuperset({1, 2})
assert not {1, 2}.issuperset({1, 2, 3})

# isdisjoint
assert {1, 2}.isdisjoint({3, 4})
assert not {1, 2}.isdisjoint({2, 3})

print("set methods OK")
