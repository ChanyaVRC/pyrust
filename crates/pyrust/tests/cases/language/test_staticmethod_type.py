# Parity fixture: staticmethod and classmethod preserve wrapper type.
# Regression for issue #1253 — pyrust was storing the raw function and
# discarding the wrapper object.

class C:
    @staticmethod
    def sf(x):
        return x

    @classmethod
    def cf(cls, x):
        return x


# Type names via __dict__ (no descriptor protocol applied)
print(type(C.__dict__["sf"]).__name__)  # staticmethod
print(type(C.__dict__["cf"]).__name__)  # classmethod

# Direct construction
sm = staticmethod(lambda x: x)
cm = classmethod(lambda cls, x: x)
print(type(sm).__name__)   # staticmethod
print(type(cm).__name__)   # classmethod

# __func__ attribute access
f = lambda x: x
sm2 = staticmethod(f)
print(sm2.__func__ is f)   # True

g = lambda cls, x: x
cm2 = classmethod(g)
print(cm2.__func__ is g)   # True

# isinstance checks
print(isinstance(sm, staticmethod))   # True
print(isinstance(cm, classmethod))    # True

# type identity
print(type(sm) is staticmethod)   # True
print(type(cm) is classmethod)    # True

# Calling still works correctly
print(C.sf(42))   # 42
print(C.cf(42))   # 42

c = C()
print(c.sf(99))   # 99
print(c.cf(99))   # 99

# __func__ on the raw descriptor from __dict__
print(C.__dict__["sf"].__func__(7))   # 7

# hasattr checks
print(hasattr(sm, "__func__"))   # True
print(hasattr(cm, "__func__"))   # True
print(hasattr(sm, "__get__"))    # True
print(hasattr(cm, "__get__"))    # True

# Double wrapping: staticmethod(staticmethod(f)) creates a fresh object
h = lambda x: x
sm3 = staticmethod(h)
sm4 = staticmethod(sm3)
print(sm4 is sm3)            # False
print(sm4.__func__ is sm3)   # True
print(type(sm4.__func__).__name__)   # staticmethod
