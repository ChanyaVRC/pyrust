"""Parity fixture for issue #1528: type(bound_method) returns a real PyClass
singleton, not a BuiltinFunction sentinel.

CPython 3.12 behaviour being verified:
- type(c.m).__name__ == "method"
- type(type(c.m)) is type
- type(c.m) is shared across all bound methods (singleton)
- isinstance(c.m, type(c.m)) works
- type(lambda: None).__name__ == "function" (same fix applied to UserFunction)
"""


class C:
    def m(self): pass
    def m2(self): pass

    @classmethod
    def cm(cls): pass

    @staticmethod
    def sm(): pass


class D:
    def d(self): pass


c = C()
d = D()

# --- method type ---
print(type(c.m).__name__)              # method
print(type(c.m).__module__)            # builtins
print(type(type(c.m)) is type)        # True
print(type(type(c.m)).__name__)       # type

# Singleton: same class for all bound methods
print(type(c.m) is type(c.m))         # True
print(type(c.m) is type(c.m2))        # True
print(type(c.m) is type(d.d))         # True

# isinstance check
print(isinstance(c.m, type(c.m)))     # True

# classmethod produces a method object too
print(type(C.cm).__name__)             # method
print(type(C.cm) is type(c.m))        # True

# --- function type ---
f = lambda: None

print(type(f).__name__)               # function
print(type(f).__module__)             # builtins
print(type(type(f)) is type)         # True
print(type(type(f)).__name__)        # type

# Singleton: same class for all plain functions
def g(): pass

print(type(f) is type(f))            # True
print(type(f) is type(g))            # True
print(type(f) is type(lambda: None)) # True

# isinstance check
print(isinstance(f, type(f)))        # True

# staticmethod: returns the plain function, so type is function
print(type(C.sm).__name__)            # function
print(type(C.sm) is type(f))         # True

# method and function are distinct types
print(type(c.m) is type(f))          # False

# Neither type is subclassable (CPython: Py_TPFLAGS_BASETYPE not set)
try:
    class SubMethod(type(c.m)): pass
    print("no error")
except TypeError as e:
    print(str(e))

try:
    class SubFunction(type(f)): pass
    print("no error")
except TypeError as e:
    print(str(e))
