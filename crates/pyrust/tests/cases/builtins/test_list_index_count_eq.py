# Parity fixture for list.index / list.count / tuple.index / tuple.count
# using user-defined __eq__ (issue #1639).
#
# CPython dispatches __eq__ for equality checks in .index() and .count().
# Pyrust was using Rc::ptr_eq (object identity) instead, so distinct objects
# with equal values were not found.

class C:
    def __init__(self, v):
        self.v = v

    def __eq__(self, other):
        return isinstance(other, C) and self.v == other.v

    def __hash__(self):
        return hash(self.v)

    def __repr__(self):
        return f"C({self.v!r})"


# --- Happy path: distinct objects, equal value ---

a, b = C(1), C(1)

lst = [a]
print(lst.index(b))   # 0
print(lst.count(b))   # 1

tpl = (a,)
print(tpl.index(b))   # 0
print(tpl.count(b))   # 1


# --- Error path: item not present raises ValueError ---

c = C(99)
try:
    lst.index(c)
except ValueError:
    print("ValueError")

try:
    tpl.index(c)
except ValueError as e:
    print(type(e).__name__, str(e))


# --- Primitives still work correctly ---

nums = [1, 2, 3, 2]
print(nums.index(2))       # 1
print(nums.count(2))       # 2
print(nums.index(2, 2))    # 3  (start=2)
print(nums.index(2, 0, 2)) # 1  (stop=2)

tnums = (1, 2, 3, 2)
print(tnums.index(2))      # 1
print(tnums.count(2))      # 2


# --- start/stop slice args work with custom __eq__ ---

objs = [C(0), C(1), C(2), C(1)]
print(objs.index(C(1), 2))    # 3  (start=2)
print(objs.count(C(1)))        # 2

tobjs = (C(0), C(1), C(2), C(1))
print(tobjs.index(C(1), 2))   # 3
print(tobjs.count(C(1)))       # 2


# --- Empty list / tuple ---

print([].count(C(1)))    # 0
print(().count(C(1)))    # 0

try:
    [].index(C(1))
except ValueError:
    print("ValueError")

try:
    ().index(C(1))
except ValueError as e:
    print(type(e).__name__, str(e))


# --- Inverted window (start > stop after normalisation) yields empty search ---

lst2 = [C(1), C(2)]
try:
    lst2.index(C(1), 5, 0)
except ValueError:
    print("ValueError")


# --- count with multiple equal items ---

multi = [C(1), C(2), C(1), C(1), C(2)]
print(multi.count(C(1)))   # 3
print(multi.count(C(2)))   # 2
print(multi.count(C(3)))   # 0
