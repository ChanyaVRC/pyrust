# Parity fixture: map() and filter() are lazy iterators over generators.
# Issue #1388: pyrust was eagerly exhausting generator sources at construction
# time.  CPython 3.12 guarantees no elements are consumed until next() is
# called on the returned iterator object.

# --- map() laziness ---

steps = []


def tracked_gen():
    for i in range(4):
        steps.append(f"gen:{i}")
        yield i


steps.append("before_map")
m = map(lambda x: x * 2, tracked_gen())
steps.append("after_map")
v = next(m)
steps.append(f"got:{v}")
print(steps)  # ['before_map', 'after_map', 'gen:0', 'got:0']

# --- filter() laziness ---

steps2 = []


def tracked_gen2():
    for i in range(4):
        steps2.append(f"gen:{i}")
        yield i


steps2.append("before_filter")
f = filter(lambda x: x > 0, tracked_gen2())
steps2.append("after_filter")
v2 = next(f)
steps2.append(f"got:{v2}")
print(steps2)  # ['before_filter', 'after_filter', 'gen:0', 'gen:1', 'got:1']

# --- Functional correctness ---

print(list(map(lambda x: x * 2, range(5))))  # [0, 2, 4, 6, 8]
print(list(filter(lambda x: x > 2, range(5))))  # [3, 4]
print(next(map(str, [1, 2, 3])))  # 1

# --- filter with None (identity test) ---

print(list(filter(None, [0, 1, False, True, "", "hello"])))  # [1, True, 'hello']

# --- map with multiple iterables ---

print(list(map(lambda x, y: x + y, [1, 2, 3], [10, 20, 30])))  # [11, 22, 33]

# map stops at the shortest iterable
print(list(map(lambda x, y: x + y, [1, 2, 3], [10, 20])))  # [11, 22]

# --- Infinite generator: construction must not hang ---

def inf_gen():
    n = 0
    while True:
        yield n
        n += 1


m_inf = map(lambda x: x * 2, inf_gen())
print(next(m_inf))  # 0
print(next(m_inf))  # 2
print(next(m_inf))  # 4

# --- StopIteration on exhausted map/filter ---

m_ex = map(lambda x: x, [1, 2])
print(next(m_ex))  # 1
print(next(m_ex))  # 2
try:
    next(m_ex)
    print("MISSING StopIteration")
except StopIteration:
    print("StopIteration raised correctly")

f_ex = filter(lambda x: x > 0, [1])
print(next(f_ex))  # 1
try:
    next(f_ex)
    print("MISSING StopIteration")
except StopIteration:
    print("StopIteration raised correctly")

# --- type() returns correct names ---

print(type(map(str, [])).__name__)   # map
print(type(filter(None, [])).__name__)  # filter
