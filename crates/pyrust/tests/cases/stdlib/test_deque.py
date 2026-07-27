# Parity fixture for collections.deque.
#
# Tests the full deque API against CPython 3.12 semantics.

from collections import deque

# ── construction ──────────────────────────────────────────────────────────────

print(list(deque()))                     # []
print(list(deque([1, 2, 3])))           # [1, 2, 3]
print(list(deque([1, 2, 3], 2)))        # [2, 3]  — initial trim to maxlen

d = deque(maxlen=5)
print(d.maxlen)                          # 5
d = deque()
print(d.maxlen)                          # None
d = deque([1, 2, 3], maxlen=2)
print(d.maxlen, list(d))                 # 2 [2, 3]

# ── append / appendleft ───────────────────────────────────────────────────────

d = deque([1, 2, 3], maxlen=3)
d.append(4)
print(list(d))                           # [2, 3, 4]

d = deque([1, 2, 3], maxlen=3)
d.appendleft(0)
print(list(d))                           # [0, 1, 2]

d = deque(maxlen=0)
d.append(99)
d.appendleft(99)
print(list(d))                           # []

# ── pop / popleft ─────────────────────────────────────────────────────────────

d = deque([1, 2, 3])
print(d.pop(), list(d))                  # 3 [1, 2]
print(d.popleft(), list(d))             # 1 [2]

try:
    deque().pop()
except IndexError as e:
    print(e)                             # pop from an empty deque

try:
    deque().popleft()
except IndexError as e:
    print(e)                             # pop from an empty deque

# ── extend / extendleft ───────────────────────────────────────────────────────

d = deque([1, 2])
d.extend([3, 4, 5])
print(list(d))                           # [1, 2, 3, 4, 5]

d = deque(maxlen=3)
d.extend([1, 2, 3, 4, 5])
print(list(d))                           # [3, 4, 5]

d = deque([1, 2, 3])
d.extendleft([4, 5])
print(list(d))                           # [5, 4, 1, 2, 3]  — reversed

d = deque(maxlen=3)
d.extendleft([1, 2, 3, 4, 5])
print(list(d))                           # [5, 4, 3]

d = deque(maxlen=0)
d.extend([1, 2, 3])
d.extendleft([4, 5])
print(list(d))                           # []

# ── rotate ────────────────────────────────────────────────────────────────────

d = deque([1, 2, 3])
d.rotate()          # default n=1
print(list(d))                           # [3, 1, 2]

d = deque([1, 2, 3])
d.rotate(1)
print(list(d))                           # [3, 1, 2]

d = deque([1, 2, 3])
d.rotate(-1)
print(list(d))                           # [2, 3, 1]

d = deque([1, 2, 3, 4, 5])
d.rotate(2)
print(list(d))                           # [4, 5, 1, 2, 3]

d = deque([1, 2, 3])
d.rotate(0)
print(list(d))                           # [1, 2, 3]

d = deque([])
d.rotate(3)
print(list(d))                           # []

# ── clear ─────────────────────────────────────────────────────────────────────

d = deque([1, 2, 3], maxlen=5)
d.clear()
print(list(d), d.maxlen)                # [] 5

# ── copy ──────────────────────────────────────────────────────────────────────

d = deque([1, 2, 3], maxlen=5)
d2 = d.copy()
print(list(d2), d2.maxlen)             # [1, 2, 3] 5

# ── count ─────────────────────────────────────────────────────────────────────

d = deque([1, 2, 2, 3, 2])
print(d.count(2))                        # 3
print(d.count(9))                        # 0

# ── remove ────────────────────────────────────────────────────────────────────

d = deque([1, 2, 3, 2, 1])
d.remove(2)
print(list(d))                           # [1, 3, 2, 1]

try:
    deque([1, 2, 3]).remove(5)
except ValueError as e:
    print(e)                             # 5 is not in deque

# ── reverse ───────────────────────────────────────────────────────────────────

d = deque([1, 2, 3])
d.reverse()
print(list(d))                           # [3, 2, 1]

# ── index ─────────────────────────────────────────────────────────────────────

d = deque([1, 2, 3, 2, 1])
print(d.index(2))                        # 1
print(d.index(2, 2))                    # 3

try:
    d.index(9)
except ValueError as e:
    print(e)                             # 9 is not in deque

try:
    d.index(1, 1, 4)
except ValueError as e:
    print(e)                             # 1 is not in deque

# ── insert ────────────────────────────────────────────────────────────────────

d = deque([1, 2, 3])
d.insert(1, 10)
print(list(d))                           # [1, 10, 2, 3]

d = deque([1, 2, 3])
d.insert(0, 0)
print(list(d))                           # [0, 1, 2, 3]

d = deque([1, 2, 3])
d.insert(100, 99)
print(list(d))                           # [1, 2, 3, 99]

d = deque([1, 2, 3])
d.insert(-1, 99)
print(list(d))                           # [1, 2, 99, 3]

try:
    deque([1, 2, 3], maxlen=3).insert(1, 10)
except IndexError as e:
    print(e)                             # deque already at its maximum size

# ── __getitem__ / __setitem__ / __delitem__ ───────────────────────────────────

d = deque([1, 2, 3])
print(d[0], d[-1], d[1])               # 1 3 2

d[1] = 99
print(list(d))                           # [1, 99, 3]

del d[1]
print(list(d))                           # [1, 3]

try:
    d = deque([1, 2, 3])
    _ = d[10]
except IndexError as e:
    print(e)                             # deque index out of range

try:
    d = deque([1, 2, 3])
    _ = d[-10]
except IndexError as e:
    print(e)                             # deque index out of range

# ── __contains__ ─────────────────────────────────────────────────────────────

d = deque([1, 2, 3])
print(2 in d)                            # True
print(5 in d)                            # False

# ── __iter__ ─────────────────────────────────────────────────────────────────

d = deque([1, 2, 3])
print(list(d))                           # [1, 2, 3]
# Normal, unmodified iteration traverses the live deque in order.
for x in d:
    pass

# ── __len__ ───────────────────────────────────────────────────────────────────

print(len(deque()))                      # 0
print(len(deque([1, 2, 3])))            # 3

# ── __repr__ ─────────────────────────────────────────────────────────────────

print(repr(deque()))                     # deque([])
print(repr(deque([1, 2, 3])))          # deque([1, 2, 3])
print(repr(deque([1, 2, 3], maxlen=5)))  # deque([1, 2, 3], maxlen=5)
print(repr(deque(maxlen=3)))             # deque([], maxlen=3)

# Nested deques must use their __repr__, not the fallback object repr.
print(repr(deque([deque([1, 2]), 3])))  # deque([deque([1, 2]), 3])

# ── __setattr__ — maxlen is read-only, no instance __dict__ ──────────────────

try:
    deque().maxlen = 5
except AttributeError as e:
    print(e)                             # attribute 'maxlen' of '...' is not writable

try:
    deque().custom = 5
except AttributeError as e:
    print(e)                             # 'collections.deque' object has no attribute 'custom'

# ── __eq__ ───────────────────────────────────────────────────────────────────

print(deque([1, 2, 3]) == deque([1, 2, 3]))   # True
print(deque([1, 2, 3]) == deque([1, 2]))      # False
print(deque([1, 2, 3]) == [1, 2, 3])          # False
print(deque([1, 2, 3]) == deque([1, 2, 4]))   # False
