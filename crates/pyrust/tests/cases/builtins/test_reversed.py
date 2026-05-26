# Tests for reversed() — sequence types succeed, non-reversible iterators raise TypeError.

# --- Reversible sequences ---
print(list(reversed([1, 2, 3])))        # [3, 2, 1]
print(list(reversed((10, 20, 30))))     # [30, 20, 10]
print(list(reversed(range(5))))         # [4, 3, 2, 1, 0]
print(list(reversed(b"abc")))           # [99, 98, 97]

# --- Generator is not reversible ---
try:
    gen = (x * 2 for x in range(5))
    reversed(gen)
    print("FAIL: generator should raise TypeError")
except TypeError as e:
    print("OK: generator TypeError")

# --- list_iterator (iter(list)) is not reversible ---
try:
    it = iter([1, 2, 3])
    reversed(it)
    print("FAIL: list iterator should raise TypeError")
except TypeError as e:
    print("OK: list iterator TypeError")

# --- Generator is not consumed by the failed reversed() call ---
items = []
gen2 = (x for x in [10, 20, 30])
try:
    reversed(gen2)
except TypeError:
    pass
for v in gen2:
    items.append(v)
print(items)  # [10, 20, 30] — generator not exhausted

# --- User object with __reversed__ is accepted ---
class MyReversed:
    def __reversed__(self):
        return iter([99, 88, 77])

print(list(reversed(MyReversed())))  # [99, 88, 77]

# --- User object with __len__ + __getitem__ is accepted (sequence protocol) ---
class MySeq:
    def __len__(self):
        return 3
    def __getitem__(self, i):
        return i * 10

print(list(reversed(MySeq())))  # [20, 10, 0]

# --- User object with only __getitem__ raises TypeError (no len) ---
class NoLen:
    def __getitem__(self, i):
        return i

try:
    reversed(NoLen())
    print("FAIL: NoLen should raise TypeError")
except TypeError as e:
    print("OK: no-len TypeError")

# --- User object with neither raises TypeError ---
class Plain:
    pass

try:
    reversed(Plain())
    print("FAIL: Plain should raise TypeError")
except TypeError as e:
    print("OK: plain TypeError")
