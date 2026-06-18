# Issue #2550: `__call__` is exposed on plain functions, lambdas, and builtins.


def f(a, b):
    return a + b


# hasattr / getattr finds __call__ on a def-defined function and a lambda.
print(hasattr(f, "__call__"))
g = lambda a, b: a + b
print(hasattr(g, "__call__"))

# Builtin function and method descriptor.
print(hasattr(len, "__call__"))
print(hasattr(str.upper, "__call__"))

# The wrapper is a method-wrapper, callable, and delegates to the underlying
# callable.
print(type(f.__call__).__name__)
print(callable(f.__call__))
print(callable(len.__call__))
print(f.__call__(1, 2))
print(g.__call__(3, 4))
print(len.__call__([1, 2, 3]))
print(str.upper.__call__("hi"))

# __self__ is the bound callable, identity preserved.
print(f.__call__.__self__ is f)

# Repr prefix (address is implementation-specific, so strip it).
print(repr(f.__call__).rsplit(" at ", 1)[0])
print(repr(len.__call__).rsplit(" at ", 1)[0])
print(repr(str.upper.__call__).rsplit(" at ", 1)[0])

# staticmethod is callable (PEP-3155, 3.12); classmethod object is not.
print(hasattr(staticmethod(f), "__call__"))
print(hasattr(classmethod(f), "__call__"))


# Bound methods (instance, classmethod-bound) and builtin bound methods are
# also callable, so they expose `__call__` too.
class C:
    @classmethod
    def cm(cls):
        return "cm"

    @staticmethod
    def sm():
        return "sm"

    def m(self):
        return "m"


c = C()
print(hasattr(C.cm, "__call__"))
print(hasattr(C.sm, "__call__"))
print(hasattr(C.m, "__call__"))
print(hasattr(c.cm, "__call__"))
print(hasattr(c.m, "__call__"))
print(C.cm.__call__())
print(c.m.__call__())
print(type(c.m.__call__).__name__)
print(repr(c.m.__call__).rsplit(" at ", 1)[0])
print(repr(C.cm.__call__).rsplit(" at ", 1)[0])

# Builtin bound method (`list.append` bound to an instance).
lst = [1, 2]
print(hasattr(lst.append, "__call__"))
lst.append.__call__(3)
print(lst)
print(repr(lst.append.__call__).rsplit(" at ", 1)[0])
