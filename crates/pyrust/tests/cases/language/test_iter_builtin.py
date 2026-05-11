it = iter([1, 2, 3])
assert next(it) == 1
assert next(it) == 2
assert next(it) == 3
try:
    next(it)
    assert False, "should raise StopIteration"
except StopIteration:
    pass

# next() with default
it2 = iter([10])
assert next(it2, 99) == 10
assert next(it2, 99) == 99

# iter on tuple
assert list(iter((4, 5, 6))) == [4, 5, 6]

# iter on string
assert list(iter("abc")) == ['a', 'b', 'c']

# iter on dict → yields keys
d = {"x": 1, "y": 2}
keys = list(iter(d))
assert keys == ["x", "y"]

# iter on set
s = {7, 8, 9}
assert sorted(list(iter(s))) == [7, 8, 9]

# iter on range
assert list(iter(range(4))) == [0, 1, 2, 3]

# iter on custom object with __iter__
class Counter:
    def __init__(self, n):
        self.n = n
        self.i = 0
    def __iter__(self):
        return self
    def __next__(self):
        if self.i >= self.n:
            raise StopIteration
        self.i += 1
        return self.i

vals = list(Counter(3))
assert vals == [1, 2, 3]

# iter() on an object whose __iter__ returns self
c = Counter(2)
assert iter(c) is c

# TypeError for non-iterable
try:
    iter(42)
    assert False, "should raise TypeError"
except TypeError:
    pass

print("iter builtin OK")
