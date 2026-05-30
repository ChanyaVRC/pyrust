# Parity fixture: user-defined __iter__ / __next__ protocol.
# Exercises for-loops, list/tuple/set conversion, isinstance with ABC,
# separate iterable+iterator classes, and error paths.

# ── Basic: object is its own iterator (returns self from __iter__) ──────────

class Counter:
    def __init__(self, stop):
        self.i = 0
        self.stop = stop

    def __iter__(self):
        return self

    def __next__(self):
        if self.i >= self.stop:
            raise StopIteration
        v = self.i
        self.i += 1
        return v


for x in Counter(3):
    print(x)  # 0, 1, 2

print(list(Counter(3)))    # [0, 1, 2]
print(tuple(Counter(3)))   # (0, 1, 2)
print(sorted(Counter(3)))  # [0, 1, 2]

# ── Separate iterable and iterator classes ───────────────────────────────────

class MyRange:
    def __init__(self, n):
        self.n = n

    def __iter__(self):
        return MyRangeIter(0, self.n)


class MyRangeIter:
    def __init__(self, start, stop):
        self.i = start
        self.n = stop

    def __iter__(self):
        return self

    def __next__(self):
        if self.i >= self.n:
            raise StopIteration
        v = self.i
        self.i += 1
        return v


for x in MyRange(3):
    print(x)  # 0, 1, 2

# Fresh iterator on each call to __iter__
r = MyRange(3)
print(list(r))  # [0, 1, 2]
print(list(r))  # [0, 1, 2]  (not exhausted, each list() call gets a fresh iter)

# ── isinstance checks via collections.abc ───────────────────────────────────

from collections.abc import Iterable, Iterator

print(isinstance(Counter(3), Iterable))   # True (__iter__ present)
print(isinstance(Counter(3), Iterator))   # True (__iter__ + __next__ present)
print(isinstance(MyRange(3), Iterable))   # True (__iter__ present)
print(isinstance(MyRange(3), Iterator))   # False (no __next__ on MyRange itself)
print(isinstance(MyRangeIter(0, 3), Iterator))  # True

# ── Empty iterator: StopIteration raised immediately ────────────────────────

class Empty:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration


print(list(Empty()))  # []
ran = False
for _ in Empty():
    ran = True
print(ran)  # False

# ── Error in __next__ propagates ────────────────────────────────────────────

class BurstOnTwo:
    def __init__(self):
        self.i = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.i == 2:
            raise ValueError("burst")
        v = self.i
        self.i += 1
        return v


try:
    for x in BurstOnTwo():
        print(x)  # 0, 1
except ValueError as e:
    print(e)  # burst

# ── __iter__ returning non-iterator raises TypeError ────────────────────────

class BadIter:
    def __iter__(self):
        return 42


try:
    for _ in BadIter():
        pass
except TypeError as e:
    print(e)  # iter() returned non-iterator of type 'int'

# ── Single-element iterator ──────────────────────────────────────────────────

class One:
    def __init__(self, val):
        self.val = val
        self.done = False

    def __iter__(self):
        return self

    def __next__(self):
        if self.done:
            raise StopIteration
        self.done = True
        return self.val


print(list(One(42)))  # [42]
for x in One("hello"):
    print(x)  # hello
