# Parity test: str.join() accepts any iterable, not just list/tuple/str/dict.
# Covers: custom __iter__/__next__, iter(), empty iterables, error messages.

# ── Fast paths (must keep working) ──────────────────────────────────────────

print(''.join(['a', 'b', 'c']))       # list
print('|'.join(('x', 'y', 'z')))     # tuple
print('-'.join('abc'))                # str (chars)
print(','.join({'k1': 1, 'k2': 2}))  # dict (keys)

# ── iter() built-in wrapping a list ─────────────────────────────────────────

print('-'.join(iter(['a', 'b', 'c'])))

# ── Custom __iter__ / __next__ class ────────────────────────────────────────

class MyIter:
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

print(''.join(MyIter(['a', 'b', 'c'])))
print(', '.join(MyIter(['1', '2', '3'])))

# ── Empty iterables ──────────────────────────────────────────────────────────

print(repr(''.join([])))
print(repr(''.join(iter([]))))
print(repr(''.join(MyIter([]))))

# ── Non-str element raises TypeError with correct message ────────────────────

try:
    ''.join([1, 2, 3])
except TypeError as e:
    print(e)

try:
    ''.join(['a', 2, 'c'])
except TypeError as e:
    print(e)

try:
    ''.join(MyIter([1]))
except TypeError as e:
    print(e)

# ── Non-iterable raises TypeError ───────────────────────────────────────────

try:
    ''.join(42)
except TypeError as e:
    print(e)

try:
    ''.join(None)
except TypeError as e:
    print(e)

# ── Wrong argument count raises TypeError ────────────────────────────────────

try:
    ''.join()
except TypeError as e:
    print(e)

try:
    ''.join(['a'], ['b'])
except TypeError as e:
    print(e)
