"""
Parity fixture for the `in` / `not in` operator using user-defined __eq__.

CPython dispatches __eq__ for membership tests on list and tuple; pyrust
previously fell back to object identity (Rc::ptr_eq) for PyInstance values.
"""


class C:
    def __init__(self, v):
        self.v = v

    def __eq__(self, other):
        return isinstance(other, C) and self.v == other.v

    def __hash__(self):
        return hash(self.v)


a, b = C(1), C(1)

# basic identity/equality distinction
print(a is b)        # False — distinct objects
print(a == b)        # True  — same value via __eq__

# list membership using __eq__
print(a in [b])      # True
print(b in [a])      # True
print(a in [a])      # True (identity → __eq__ short-circuit)
print(a not in [b])  # False

# tuple membership using __eq__
print(a in (b,))     # True
print(b in (a,))     # True
print(a in (a,))     # True
print(a not in (b,)) # False

# false case: genuinely different value
print(C(2) in [a, b])   # False
print(C(2) in (a, b))   # False

# primitives unaffected (regression check)
print(1 in [1, 2, 3])   # True
print(4 in [1, 2, 3])   # False
print("x" in ("x", "y"))  # True
print(None in [None])   # True
print(None in [1, 2])   # False

# __eq__ raising an exception propagates
class Boom:
    def __eq__(self, other):
        raise RuntimeError("boom")

try:
    _ = Boom() in [Boom()]
except RuntimeError as e:
    print("RuntimeError:", e)


# User iterable via __iter__ (not __contains__) also uses __eq__
class MyList:
    def __init__(self, items):
        self._items = items

    def __iter__(self):
        return iter(self._items)


ml = MyList([a, C(2)])
print(b in ml)      # True  (b == a via __eq__, dispatched through __iter__)
print(C(3) in ml)   # False


# Legacy __getitem__-sequence protocol also uses __eq__
class MySeq:
    def __init__(self, items):
        self._items = items

    def __getitem__(self, i):
        if i >= len(self._items):
            raise IndexError(i)
        return self._items[i]


ms = MySeq([a, C(2)])
print(b in ms)      # True
print(C(3) in ms)   # False
