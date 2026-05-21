# Parity fixture: `object` builtin (#831) and `sys.maxsize` /
# `sys.version_info` / `sys.platform` (#833).

# ── object builtin ──────────────────────────────────────────────────────────

o = object()
print(isinstance(o, object))       # True — object instance is an instance of object
print(isinstance(42, object))      # True — int is a subtype of object
print(isinstance("hi", object))    # True — str is a subtype of object
print(isinstance(None, object))    # True — NoneType is a subtype of object
print(issubclass(int, object))     # True
print(issubclass(str, object))     # True
print(callable(object))            # True — object is a class, hence callable
print(type(o).__name__)            # object

# User-defined classes are also subclasses of object.
class MyClass:
    pass

obj = MyClass()
print(isinstance(obj, object))     # True
print(issubclass(MyClass, object)) # True

# ── sys.maxsize ──────────────────────────────────────────────────────────────

import sys

print(sys.maxsize > 0)             # True
print(sys.maxsize == 2**63 - 1)    # True  (always true on 64-bit)
print(type(sys.maxsize).__name__)  # int

# ── sys.version_info ─────────────────────────────────────────────────────────

print(sys.version_info.major)      # 3
print(isinstance(sys.version_info.minor, int))     # True
print(isinstance(sys.version_info.micro, int))     # True
print(sys.version_info.releaselevel)               # final
print(type(sys.version_info).__qualname__)         # version_info

# ── sys.platform ─────────────────────────────────────────────────────────────

print(isinstance(sys.platform, str))  # True
print(len(sys.platform) > 0)          # True
