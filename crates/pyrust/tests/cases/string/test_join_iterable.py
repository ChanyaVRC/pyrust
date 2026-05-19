# str.join() must accept any iterable, not just list/tuple/str/dict.
# Regression test for issue #576.

# --- fast paths (list, tuple, str) ---
print(''.join(['a', 'b', 'c']))          # abc
print(''.join(('x', 'y')))              # xy
print('-'.join('abc'))                  # a-b-c

# --- iter() wrapping a list ---
print(''.join(iter(['a', 'b', 'c'])))   # abc
print(', '.join(iter(['p', 'q'])))      # p, q

# --- generator function ---
def letters():
    yield 'a'
    yield 'b'

print(''.join(letters()))               # ab
print('-'.join(letters()))              # a-b

# --- custom __iter__/__next__ class ---
class StrIter:
    def __init__(self, items):
        self.items = items
        self.i = 0
    def __iter__(self):
        return self
    def __next__(self):
        if self.i >= len(self.items):
            raise StopIteration
        v = self.items[self.i]
        self.i += 1
        return v

print(''.join(StrIter(['h', 'i'])))     # hi
print(', '.join(StrIter(['x', 'y', 'z'])))  # x, y, z

# --- error: non-iterable ---
try:
    ''.join(42)
except TypeError as e:
    print(e)                             # can only join an iterable

# --- error: non-str element in list ---
try:
    ''.join([1, 2])
except TypeError as e:
    print(e)                             # sequence item 0: expected str instance, int found

# --- error: non-str element in middle ---
try:
    ''.join(['a', 1, 'c'])
except TypeError as e:
    print(e)                             # sequence item 1: expected str instance, int found

# --- error: non-str element from custom iterable ---
class IntIter:
    def __init__(self):
        self.i = 0
    def __iter__(self):
        return self
    def __next__(self):
        if self.i >= 3:
            raise StopIteration
        v = self.i
        self.i += 1
        return v

try:
    ''.join(IntIter())
except TypeError as e:
    print(e)                             # sequence item 0: expected str instance, int found

# --- error: TypeError raised inside __next__ must propagate unchanged ---
# Regression for #576: join must NOT rewrite user TypeErrors to
# "can only join an iterable".
class RaisingIter:
    def __iter__(self):
        return self
    def __next__(self):
        raise TypeError("custom error from __next__")

try:
    ''.join(RaisingIter())
except TypeError as e:
    print(e)                             # custom error from __next__

# --- error: TypeError raised inside a generator body must propagate ---
def bad_gen():
    raise TypeError("custom error from generator")
    yield 'x'

try:
    ''.join(bad_gen())
except TypeError as e:
    print(e)                             # custom error from generator
