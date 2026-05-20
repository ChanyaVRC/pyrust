"""Parity: ForIter __next__ method cache — slot reuse across loops."""

class Counter:
    def __init__(self, start, stop):
        self.cur = start
        self.stop = stop

    def __iter__(self):
        return self

    def __next__(self):
        if self.cur >= self.stop:
            raise StopIteration
        v = self.cur
        self.cur += 1
        return v


class Squares:
    def __init__(self, n):
        self.n = n
        self.i = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.i >= self.n:
            raise StopIteration
        v = self.i * self.i
        self.i += 1
        return v


# Two different classes reusing the same loop slot must not share the cache.
result1 = list(Counter(0, 5))
print(result1)             # [0, 1, 2, 3, 4]

result2 = list(Squares(5))
print(result2)             # [0, 1, 4, 9, 16]

# Multiple Counter loops back-to-back — cache should persist across iterations.
total = 0
for x in Counter(1, 6):
    total += x
print(total)               # 15

total2 = 0
for x in Counter(0, 4):
    total2 += x
print(total2)              # 6
