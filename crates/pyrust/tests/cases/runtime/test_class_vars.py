# Issue #553: vars(C) must not include __qualname__ for a plain class.
# CPython stores __qualname__ as a type-level descriptor on `type`, not in
# the class's own __dict__.  __module__ IS stored in the class dict.

class C:
    pass

print('__qualname__' in vars(C))   # False
print('__module__' in vars(C))     # True
print(C.__qualname__)              # C
print(C.__module__)                # __main__

# User-assigned __qualname__ inside the class body is captured and removed
# from attrs, but still accessible via C.__qualname__.
class D:
    __qualname__ = 'CustomD'

print('__qualname__' in vars(D))   # False
print(D.__qualname__)              # CustomD

# User-assigned __module__ stays in attrs.
class E:
    __module__ = 'mymodule'

print('__module__' in vars(E))     # True
print(E.__module__)                # mymodule

# Subclass: __qualname__ not in vars(), attribute access works.
class Base:
    pass

class Child(Base):
    pass

print('__qualname__' in vars(Child))   # False
print(Child.__qualname__)              # Child

# Class with real attrs: only the user-defined attrs appear in vars().
class F:
    x = 1
    y = 2

keys = list(vars(F).keys())
print('__qualname__' in keys)   # False
print('x' in keys)              # True
print('y' in keys)              # True
