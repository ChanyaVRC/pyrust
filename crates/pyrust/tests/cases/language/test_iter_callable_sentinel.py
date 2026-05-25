# Parity fixture for iter(callable, sentinel) — the two-argument form.
# The one-argument form is covered by test_builtin_subclass_iter.py.

# Basic use: count up to (not including) sentinel.
count = [0]
def gen():
    count[0] += 1
    return count[0]

result = list(iter(gen, 4))
print(result)  # [1, 2, 3]

# Lambda callable with a finite source iterator.
data = iter([1, 2, 3, 0])
result2 = list(iter(lambda: next(data), 0))
print(result2)  # [1, 2, 3]

# Empty case: sentinel returned immediately yields empty sequence.
result3 = list(iter(lambda: 0, 0))
print(result3)  # []

# for-loop usage.
data2 = [10, 20, 30, 0]
idx = [0]
def get_next():
    v = data2[idx[0]]
    idx[0] += 1
    return v

result4 = []
for x in iter(get_next, 0):
    result4.append(x)
print(result4)  # [10, 20, 30]

# tuple() usage.
data3 = iter([5, 10, 15, -1])
t = tuple(iter(lambda: next(data3), -1))
print(t)  # (5, 10, 15)

# next() with default: returns default when sentinel hit, then again when done.
count2 = [0]
def gen2():
    count2[0] += 1
    return count2[0]

it = iter(gen2, 2)
print(next(it))           # 1
print(next(it, "done"))   # "done" — 2 equals sentinel
print(next(it, "done"))   # "done" — already exhausted

# next() without default raises StopIteration when exhausted.
count3 = [0]
def gen3():
    count3[0] += 1
    return count3[0]

it2 = iter(gen3, 3)
print(next(it2))  # 1
print(next(it2))  # 2
try:
    next(it2)  # 3 equals sentinel
except StopIteration:
    print("StopIteration raised correctly")

# Non-callable first argument raises TypeError.
# The exact message wording changed between CPython 3.12.3 and 3.12.13;
# only assert the stable part.
try:
    iter("not callable", "x")
except TypeError as e:
    print("TypeError: must be callable:", "must be callable" in str(e))

# Three arguments raises TypeError.
try:
    iter(gen, 4, "extra")
except TypeError as e:
    print("TypeError 3args:", e)

# Zero arguments raises TypeError.
try:
    iter()
except TypeError as e:
    print("TypeError 0args:", e)
