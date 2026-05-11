# Tests for user-defined dunder method dispatch


# ── __call__ ──────────────────────────────────────────────────────────────────

class Multiplier:
    def __init__(self, factor):
        self.factor = factor

    def __call__(self, x):
        return x * self.factor

double = Multiplier(2)
triple = Multiplier(3)
print(double(5))    # 10
print(triple(4))    # 12
print(double(0))    # 0


class Adder:
    def __init__(self, n):
        self.n = n

    def __call__(self, a, b):
        return a + b + self.n

add5 = Adder(5)
print(add5(1, 2))   # 8


# ── __bool__ ─────────────────────────────────────────────────────────────────

class AlwaysFalse:
    def __bool__(self):
        return False

class AlwaysTrue:
    def __bool__(self):
        return True

class EmptyCheck:
    def __init__(self, items):
        self.items = items

    def __len__(self):
        return len(self.items)

af = AlwaysFalse()
at_ = AlwaysTrue()
ec_empty = EmptyCheck([])
ec_full = EmptyCheck([1, 2, 3])

print(bool(af))         # False
print(bool(at_))        # True
print(bool(ec_empty))   # False
print(bool(ec_full))    # True

# if/while use __bool__
if af:
    print("WRONG")
else:
    print("af is falsy")    # af is falsy

if at_:
    print("at_ is truthy")  # at_ is truthy
else:
    print("WRONG")

# not uses __bool__
print(not af)   # True
print(not at_)  # False

# Instances with no __bool__ and no __len__ are always truthy
class Bare:
    pass

b = Bare()
print(bool(b))  # True
if b:
    print("bare is truthy")  # bare is truthy


# ── __len__ ───────────────────────────────────────────────────────────────────

class MyList:
    def __init__(self, items):
        self.items = items

    def __len__(self):
        return len(self.items)

ml0 = MyList([])
ml3 = MyList([10, 20, 30])

print(len(ml0))     # 0
print(len(ml3))     # 3


# ── __getitem__ ───────────────────────────────────────────────────────────────

class SquareBox:
    """Returns key ** 2."""

    def __getitem__(self, key):
        return key * key

sq = SquareBox()
print(sq[3])    # 9
print(sq[5])    # 25
print(sq[0])    # 0


class MappingProxy:
    def __init__(self, data):
        self.data = data

    def __getitem__(self, key):
        return self.data[key]

mp = MappingProxy({"a": 1, "b": 2})
print(mp["a"])  # 1
print(mp["b"])  # 2


# ── __setitem__ ───────────────────────────────────────────────────────────────

class Store:
    def __init__(self):
        self.data = {}

    def __setitem__(self, key, value):
        self.data[key] = value

    def __getitem__(self, key):
        return self.data[key]

s = Store()
s["x"] = 42
s["y"] = 99
print(s["x"])   # 42
print(s["y"])   # 99

s["x"] = 100
print(s["x"])   # 100


# ── __delitem__ ───────────────────────────────────────────────────────────────

class DeletableStore:
    def __init__(self):
        self.data = {}

    def __setitem__(self, key, value):
        self.data[key] = value

    def __getitem__(self, key):
        return self.data[key]

    def __delitem__(self, key):
        del self.data[key]

    def __len__(self):
        return len(self.data)

ds = DeletableStore()
ds["a"] = 1
ds["b"] = 2
ds["c"] = 3
print(len(ds))  # 3
del ds["b"]
print(len(ds))  # 2
print(ds["a"])  # 1
print(ds["c"])  # 3


# ── __iter__ / __next__ ───────────────────────────────────────────────────────

class CountUp:
    """Iterates from start to stop-1 (inclusive)."""

    def __init__(self, start, stop):
        self.start = start
        self.stop = stop

    def __iter__(self):
        return CountUpIterator(self.start, self.stop)


class CountUpIterator:
    def __init__(self, start, stop):
        self.current = start
        self.stop = stop

    def __iter__(self):
        return self

    def __next__(self):
        if self.current >= self.stop:
            raise StopIteration
        val = self.current
        self.current = self.current + 1
        return val


cu = CountUp(1, 5)
for v in cu:
    print(v)    # 1 2 3 4

# list() on a user iterable (via iter_values → hits __iter__/__next__ path via for-loop)
result = []
for x in CountUp(10, 14):
    result.append(x)
print(result)   # [10, 11, 12, 13]


class FibIterator:
    """Yields the first n Fibonacci numbers."""

    def __init__(self, n):
        self.n = n
        self.a = 0
        self.b = 1
        self.count = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.count >= self.n:
            raise StopIteration
        val = self.a
        self.a, self.b = self.b, self.a + self.b
        self.count = self.count + 1
        return val


fibs = []
for f in FibIterator(7):
    fibs.append(f)
print(fibs)     # [0, 1, 1, 2, 3, 5, 8]
