# range.start / .stop / .step read-only properties (issue #1807)
# and range.count() / range.index() / range.__len__() methods.

r = range(1, 10, 2)
print(r.start)   # 1
print(r.stop)    # 10
print(r.step)    # 2

# Single-arg range: start=0, step=1
print(range(5).start)  # 0
print(range(5).stop)   # 5
print(range(5).step)   # 1

# Empty range still exposes attributes
print(range(0, 10, -1).start)  # 0
print(range(0, 10, -1).stop)   # 10
print(range(0, 10, -1).step)   # -1

# Read-only: setting raises AttributeError
try:
    r.start = 2
except AttributeError as e:
    print(e)

try:
    r.stop = 2
except AttributeError as e:
    print(e)

try:
    r.step = 2
except AttributeError as e:
    print(e)

# Unknown attribute gives "has no attribute"
try:
    _ = r.foo
except AttributeError as e:
    print(e)

# __len__
print(r.__len__())       # 5
print(range(0).__len__())  # 0

# count: 0 or 1
print(r.count(5))   # 1
print(r.count(4))   # 0
print(r.count(0))   # 0

# index: returns position
print(r.index(1))   # 0
print(r.index(5))   # 2
print(r.index(9))   # 4

# index: raises ValueError when not found
try:
    r.index(4)
except ValueError as e:
    print(e)

# index: non-integer type gives "sequence.index(x): x not in sequence"
try:
    r.index("hello")
except ValueError as e:
    print(e)

# count / index with bool (bool is subclass of int)
print(range(0, 2).count(True))   # 1
print(range(0, 2).index(False))  # 0

# integer-valued float
print(r.index(5.0))   # 2

# arity errors carry the right prefix (CPython: "range.count()" / "range.index()")
try:
    r.count()
except TypeError as e:
    print(e)

try:
    r.index()
except TypeError as e:
    print(e)

# __len__ arity: CPython says "expected 0 arguments, got N"
try:
    r.__len__(1)
except TypeError as e:
    print(e)
