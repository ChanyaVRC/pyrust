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

print("container protocol OK")
