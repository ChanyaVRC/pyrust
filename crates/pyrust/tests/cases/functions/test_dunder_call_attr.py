# Issue #2550: every callable exposes a `__call__` attribute, so
# `hasattr(callable, "__call__")` is True and `callable.__call__(...)`
# re-dispatches the call onto the underlying callable.  The wrapper's repr
# embeds a non-deterministic address, so this fixture asserts behaviour
# (hasattr / call results / type names / __self__ / __name__) rather than repr.


# --- plain function ---
def f():
    return None


print(hasattr(f, "__call__"))  # True
print(f.__call__())  # None
print(type(f.__call__).__name__)  # method-wrapper
print(f.__call__.__name__)  # __call__
print(f.__call__.__self__ is f)  # True


# --- lambda ---
print((lambda: 42).__call__())  # 42
print((lambda x: x * 2).__call__(5))  # 10


# --- user class defining __call__ ---
class Foo:
    def __call__(self):
        return 1


foo = Foo()
print(hasattr(foo, "__call__"))  # True
print(foo.__call__())  # 1


# --- class object itself is callable via type.__call__ ---
class C:
    pass


print(hasattr(C, "__call__"))  # True
print(type(C.__call__()).__name__)  # C


# --- builtin function ---
print(hasattr(len, "__call__"))  # True
print(len.__call__([1, 2, 3]))  # 3


# --- method descriptor ---
print(hasattr(str.upper, "__call__"))  # True
print(str.upper.__call__("hi"))  # HI


# --- bound method of a user instance ---
class D:
    def m(self):
        return 7


print(hasattr(D().m, "__call__"))  # True
print(D().m.__call__())  # 7


# --- builtin bound method ---
print(hasattr([].append, "__call__"))  # True
lst = [1]
lst.append.__call__(2)
print(lst)  # [1, 2]


# --- the wrapper is itself callable (nested __call__) ---
w = f.__call__
print(type(w).__name__)  # method-wrapper
print(hasattr(w, "__call__"))  # True
print(w.__call__())  # None
print(w.__call__.__call__())  # None
print(f.__call__.__call__())  # None


# --- callable() consistency ---
print(callable(f), callable(f.__call__))  # True True
