# frozenset subset/superset comparison operators (issue #1005)
# CPython 3.12: frozenset supports <, <=, >, >= as subset/superset tests.

a = frozenset([1, 2])
b = frozenset([1, 2, 3])
empty = frozenset()

# Proper subset (<)
print(a < b)        # True
print(a < a)        # False — not a proper subset of itself
print(empty < a)    # True

# Subset (<=)
print(a <= b)       # True
print(a <= a)       # True — every set is a subset of itself
print(b <= a)       # False

# Proper superset (>)
print(b > a)        # True
print(a > a)        # False
print(a > empty)    # True

# Superset (>=)
print(b >= a)       # True
print(a >= a)       # True
print(a >= b)       # False

# Disjoint sets: neither is a subset of the other
x = frozenset([1, 2])
y = frozenset([3, 4])
print(x < y)        # False
print(x <= y)       # False
print(x > y)        # False
print(x >= y)       # False

# Mixed frozenset / set comparisons (CPython allows this)
print(frozenset([1]) < {1, 2})          # True
print({1, 2} > frozenset([1]))          # True
print({1, 2} <= frozenset([1, 2, 3]))   # True
print(frozenset([1]) >= {1})            # True
print(frozenset([1, 2]) < {1, 2})       # False — not proper subset

# Plain set comparisons still work
sa = {1, 2}
sb = {1, 2, 3}
print(sa < sb)      # True
print(sa <= sb)     # True
print(sb > sa)      # True
print(sb >= sa)     # True
print(sa < sa)      # False
print(sa <= sa)     # True

# TypeError when comparing with non-set type
try:
    print(frozenset([1]) < "hello")
except TypeError as e:
    print(type(e).__name__)

try:
    print({1} < 42)
except TypeError as e:
    print(type(e).__name__)
