"""Parity fixture: hash() dispatches __hash__; container == dispatches element
__eq__ (umbrella issue #434).

Two dispatch-boundary behaviours are locked in here:

1. ``hash(obj)`` must call the instance's ``__hash__`` method rather than
   treating every ``PyInstance`` as unhashable.  A class that defines
   ``__eq__`` but not ``__hash__`` is implicitly unhashable (CPython sets
   ``__hash__ = None``), and an explicit ``__hash__ = None`` is unhashable too.

2. ``list ==``, ``tuple ==`` and ``set ==`` must compare elements via their
   ``__eq__`` method, not by object identity.
"""


# --- hash() dispatches __hash__ ----------------------------------------------

class WithHash:
    def __hash__(self):
        return 42


print(hash(WithHash()))   # 42


class HashNone:
    __hash__ = None


try:
    hash(HashNone())
except TypeError as exc:
    print("HashNone:", exc)   # unhashable type: 'HashNone'


class EqOnly:
    # Defining __eq__ without __hash__ makes instances unhashable in CPython.
    def __eq__(self, other):
        return True


try:
    hash(EqOnly())
except TypeError as exc:
    print("EqOnly:", exc)     # unhashable type: 'EqOnly'


class HashAndEq:
    def __init__(self, k):
        self.k = k

    def __hash__(self):
        return self.k

    def __eq__(self, other):
        return isinstance(other, HashAndEq) and self.k == other.k


print(hash(HashAndEq(7)))   # 7


# --- container == dispatches element __eq__ ----------------------------------

class Always:
    def __eq__(self, other):
        return True


class Never:
    def __eq__(self, other):
        return False


# list equality via element __eq__
print([Always()] == [Always()])   # True
print([Never()] == [Never()])     # False
print([1, Always()] == [1, Always()])   # True

# tuple equality via element __eq__
print((Always(),) == (Always(),))   # True
print((Never(),) == (Never(),))     # False

# nested containers recurse into element __eq__
print([[Always()]] == [[Always()]])     # True
print([(Always(),)] == [(Always(),)])   # True

# set equality with a class defining both __hash__ and __eq__
print({HashAndEq(1), HashAndEq(2)} == {HashAndEq(2), HashAndEq(1)})   # True
print({HashAndEq(1)} == {HashAndEq(2)})   # False


# __eq__ returning NotImplemented falls back to identity comparison
class NI:
    def __eq__(self, other):
        return NotImplemented


x = NI()
print([x] == [x])         # True  — same object, identity match
print([NI()] == [NI()])   # False — distinct objects, identity mismatch
