# Parity fixture for issue #1198: object() instances must not accept attribute
# assignment.  In CPython, bare object() has no __dict__, so setattr raises
# AttributeError.  User-defined classes (even those subclassing object) still
# get their own __dict__ and must continue to work.

# --- Assignment raises AttributeError ---
obj = object()
try:
    obj.x = 1
    print("FAIL: expected AttributeError")
except AttributeError as e:
    print(e)

# --- Read of nonexistent attr also raises AttributeError ---
try:
    _ = object().missing
    print("FAIL: expected AttributeError on read")
except AttributeError as e:
    print(e)

# --- getattr with default does not raise ---
result = getattr(object(), "y", None)
print(result)  # None

# --- hasattr returns False, not True ---
print(hasattr(object(), "z"))  # False

# --- User-defined class (no explicit base) still accepts attrs ---
class Foo:
    pass

f = Foo()
f.value = 42
print(f.value)  # 42

# --- Explicit subclass of object also accepts attrs ---
class Bar(object):
    pass

b = Bar()
b.value = 99
print(b.value)  # 99

# --- object() is still hashable (identity hash) ---
a = object()
b2 = object()
print(type(hash(a)))  # <class 'int'>
print(a == a)         # True
print(a == b2)        # False

# --- sentinel pattern: two distinct sentinels are not equal ---
s1 = object()
s2 = object()
print(s1 is s2)  # False
print(s1 is s1)  # True

# --- object class itself is still accessible ---
print(object)         # <class 'object'>
print(type(object()))  # <class 'object'>
