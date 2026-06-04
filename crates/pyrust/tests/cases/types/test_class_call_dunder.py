# Parity fixture for issue #2096: a class is callable (to construct instances)
# via the metaclass slot type.__call__, so cls.__call__ resolves and
# hasattr(cls, '__call__') agrees with callable(cls).


class C:
    pass


# callable and hasattr agree for ordinary and built-in classes.
print(callable(C), hasattr(C, "__call__"))  # True True
print(callable(int), hasattr(int, "__call__"))  # True True

# C.__call__ is the type.__call__ method-wrapper, not an AttributeError.
# (The repr carries an implementation-specific address, so only the type name
# is asserted for parity stability.)
print(type(C.__call__).__name__)  # method-wrapper

# Calling it constructs an instance, exactly like C().
print(type(C.__call__()).__name__)  # C

# Primitive classes construct through __call__ too.
print(int.__call__("42"))  # 42
print(str.__call__(5))  # 5
print(list.__call__((1, 2, 3)))  # [1, 2, 3]

# A class whose __init__ takes args is constructed via __call__ with those args.
class E:
    def __init__(self, x):
        self.x = x


print(E.__call__(7).x)  # 7

# An instance does NOT spuriously gain __call__ unless its class defines one.
c = C()
print(hasattr(c, "__call__"))  # False

# A class that DEFINES __call__ (for its instances): the instance is callable,
# and C.__call__ on the class itself is that user function (not type.__call__).
class D:
    def __call__(self):
        return "inst-called"


d = D()
print(d())  # inst-called
print(type(D.__call__).__name__)  # function
print(hasattr(d, "__call__"))  # True

# Exception classes are callable and construct instances via __call__.
print(hasattr(ValueError, "__call__"))  # True
err = ValueError.__call__("boom")
print(type(err).__name__, err.args)  # ValueError ('boom',)

# object itself is callable.
print(callable(object), hasattr(object, "__call__"))  # True True

# Each access yields a fresh wrapper (identity is not preserved), matching
# CPython.
print(C.__call__ is C.__call__)  # False
