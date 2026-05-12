class SparseList:
    def __init__(self):
        self._data = {}
        self._len = 0
    def __setitem__(self, i, v):
        self._data[i] = v
        if i >= self._len:
            self._len = i + 1
    def __getitem__(self, i):
        return self._data.get(i, 0)
    def __len__(self):
        return self._len
    def __contains__(self, v):
        return v in self._data.values()
    def __delitem__(self, i):
        del self._data[i]

s = SparseList()
s[0] = 10
s[5] = 50
s[3] = 30

assert s[0] == 10
assert s[3] == 30
assert s[5] == 50
assert s[1] == 0   # default

assert len(s) == 6   # 0..=5

assert 10 in s
assert 50 in s
assert 99 not in s

del s[3]
assert s[3] == 0   # back to default

# __getitem__ for iteration via manual index
results = [s[i] for i in range(3)]
assert results == [10, 0, 0]

# __len__ truthiness: empty → falsy, non-empty → truthy
empty = SparseList()
assert not empty
assert s   # len == 6

# __bool__ takes priority over __len__
class AlwaysFalsy:
    def __bool__(self):
        return False
    def __len__(self):
        return 99

assert not AlwaysFalsy()

# __len__ used for truthiness when no __bool__
class LenOnly:
    def __init__(self, n):
        self._n = n
    def __len__(self):
        return self._n

assert not LenOnly(0)
assert LenOnly(1)
assert LenOnly(5)

# __contains__ via __iter__ fallback (no explicit __contains__)
class IterOnly:
    def __init__(self, items):
        self._items = items
    def __iter__(self):
        return iter(self._items)

io = IterOnly([1, 2, 3])
assert 2 in io
assert 4 not in io

# String keys
class StrMap:
    def __init__(self):
        self._d = {}
    def __getitem__(self, k):
        return self._d[k]
    def __setitem__(self, k, v):
        self._d[k] = v
    def __delitem__(self, k):
        del self._d[k]
    def __contains__(self, k):
        return k in self._d

m = StrMap()
m["hello"] = 42
m["world"] = 99
assert m["hello"] == 42
assert m["world"] == 99
assert "hello" in m
assert "missing" not in m
del m["hello"]
assert "hello" not in m

# TypeError for missing __getitem__
class NoGet:
    pass

try:
    _ = NoGet()[0]
    print("FAIL: expected TypeError")
except TypeError as e:
    print("no-getitem TypeError OK")

# TypeError for missing __setitem__
class NoSet:
    pass

try:
    NoSet()[0] = 1
    print("FAIL: expected TypeError")
except TypeError as e:
    print("no-setitem TypeError OK")

# TypeError for missing __delitem__
class NoDel:
    pass

try:
    del NoDel()[0]
    print("FAIL: expected TypeError")
except TypeError:
    print("no-delitem TypeError OK")

# Empty collection literals in methods (compiler fix: empty [] / () must not panic)
class WithEmptyLiterals:
    def empty_list(self):
        return []
    def empty_tuple(self):
        return ()
    def mixed(self):
        a = []
        b = ()
        a.append(1)
        return (a, b)

w = WithEmptyLiterals()
assert w.empty_list() == []
assert w.empty_tuple() == ()
mixed_result = w.mixed()
assert mixed_result[0] == [1]
assert mixed_result[1] == ()

print("container protocol OK")
