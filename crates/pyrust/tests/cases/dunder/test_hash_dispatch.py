# Parity fixture for issue #435: hash() dispatches __hash__ on PyInstance.
#
# Tests cover:
#   - callable __hash__ returning int
#   - __hash__ = None (explicitly unhashable)
#   - __hash__ inherited from a parent class
#   - __hash__ returning bool (bool is a subtype of int)
#   - __hash__ returning -1 (CPython maps this to -2)
#   - __hash__ returning non-int (TypeError)
#   - no __hash__ defined (default identity hash, returns int)
#   - dict/set construction with a custom __hash__

# 1. Basic __hash__ returning int.
class Fixed:
    def __hash__(self):
        return 42

print(hash(Fixed()))   # 42

# 2. Dict construction uses __hash__.
f = Fixed()
d = {f: "val"}
print(d[f])            # val

# 3. __hash__ = None — explicitly unhashable.
class NoHash:
    __hash__ = None

try:
    hash(NoHash())
except TypeError as e:
    print(e)           # unhashable type: 'NoHash'

# 4. Inherited __hash__.
class Parent:
    def __hash__(self):
        return 99

class Child(Parent):
    pass

print(hash(Child()))   # 99

# 5. __hash__ returning bool (bool is int).
class BoolHash:
    def __hash__(self):
        return True

print(hash(BoolHash()))  # 1

# 6. __hash__ returning -1 — CPython maps to -2.
class NegOne:
    def __hash__(self):
        return -1

print(hash(NegOne()))  # -2

# 7. __hash__ returning a non-integer raises TypeError.
class StrHash:
    def __hash__(self):
        return "oops"

try:
    hash(StrHash())
except TypeError as e:
    print(e)           # __hash__ method should return an integer

# 8. No __hash__ defined — default identity hash; result is an int.
class Plain:
    pass

print(type(hash(Plain())).__name__)  # int

# 9. Set construction deduplicates using __hash__ and __eq__.
class Key:
    def __init__(self, v):
        self.v = v
    def __hash__(self):
        return self.v
    def __eq__(self, other):
        return isinstance(other, Key) and self.v == other.v

s = {Key(1), Key(2), Key(1)}
print(len(s))          # 2
