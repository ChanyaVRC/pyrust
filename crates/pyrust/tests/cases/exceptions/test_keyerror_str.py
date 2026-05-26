# Parity fixture: KeyError.__str__ repr-quoting is inherited by subclasses.
# CPython 3.12 uses repr() of the single arg for KeyError and all subclasses.

class MyKeyError(KeyError):
    pass

class DeepKeyError(MyKeyError):
    pass

# Base KeyError: single string arg is repr-quoted
e = KeyError("missing")
print(str(e))   # 'missing'
print(repr(e))  # KeyError('missing')

# Subclass should inherit repr-quoting
e2 = MyKeyError("also missing")
print(str(e2))   # 'also missing'
print(repr(e2))  # MyKeyError('also missing')

# Deep subclass should also inherit
e3 = DeepKeyError("deep")
print(str(e3))   # 'deep'
print(repr(e3))  # DeepKeyError('deep')

# Multi-arg KeyError does NOT quote; shows tuple repr
e4 = KeyError("a", "b")
print(str(e4))   # ('a', 'b')

e5 = MyKeyError("a", "b")
print(str(e5))   # ('a', 'b')

# Non-string key: repr of the int, no extra quotes added
e6 = KeyError(42)
print(str(e6))   # 42

e7 = MyKeyError(42)
print(str(e7))   # 42

# Zero-arg
e8 = KeyError()
print(str(e8))   # (empty)

e9 = MyKeyError()
print(str(e9))   # (empty)
