class C:
    @staticmethod
    def sf(x):
        return x


# type(C.sf).__name__ must be "function", not "staticmethod"
print(type(C.sf).__name__)

# type via instance access must also be "function"
c = C()
print(type(c.sf).__name__)

# callable
print(callable(C.sf))
print(callable(c.sf))

# calling still works
print(C.sf(42))
print(c.sf(42))

# the descriptor stored in __dict__ is still "staticmethod"
print(type(C.__dict__["sf"]).__name__)

# __func__ identity is preserved
fn = lambda x: x
sm = staticmethod(fn)
print(sm.__func__ is fn)

# __get__ returns the original function object (identity)
print(sm.__get__(None, C) is fn)
print(sm.__get__(c, C) is fn)

# __get__(None, None) is a TypeError
try:
    sm.__get__(None, None)
    print("no error")
except TypeError as e:
    print("TypeError")

# staticmethod wrapping a non-function returns the value directly
sm2 = staticmethod(42)
print(C.__dict__["sf"].__func__(10))
