# Issue #2617: a bound method obtained from a @classmethod exposes the
# underlying plain function via __func__, not the classmethod wrapper.


class C:
    @classmethod
    def f(cls):
        return cls


# type of the bound method's __func__ is `function`, not `classmethod`.
print(type(C.f.__func__).__name__)

# identity: matches the descriptor's own __func__ (the original function).
print(C.f.__func__ is C.__dict__["f"].__func__)

# direct descriptor access is also a plain function.
print(type(C.__dict__["f"].__func__).__name__)

# the bound method still works when called.
print(C.f() is C)
