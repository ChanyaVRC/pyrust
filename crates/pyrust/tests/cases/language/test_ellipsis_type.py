# Parity fixture for issue #1412: type(Ellipsis) must be a proper PyClass.
# All cases verified against CPython 3.12.

print(repr(type(...)))             # <class 'ellipsis'>
print(type(...).__name__)          # ellipsis
print(isinstance(type(...), type)) # True
print(type(...) is type(...))      # True (per-thread singleton)
print(type(Ellipsis) is type(...)) # True (same singleton)
print(isinstance(..., type(...)))  # True
print(type(None) is not type(...)) # True (NoneType != ellipsis)
