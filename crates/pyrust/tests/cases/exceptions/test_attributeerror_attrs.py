# CPython 3.12 parity: AttributeError.name and AttributeError.obj

# Instance attribute miss: name and obj are set
try:
    (42).no_such_attr
except AttributeError as e:
    print(e.name)           # no_such_attr
    print(e.obj)            # 42
    print(hasattr(e, 'name'))   # True
    print(hasattr(e, 'obj'))    # True

# getattr() on a list: name and obj are set
try:
    getattr([], 'nonexistent')
except AttributeError as e:
    print(e.name)           # nonexistent
    print(e.obj)            # []

# User-constructed AttributeError: name and obj default to None
e = AttributeError('msg')
print(e.name)               # None
print(e.obj)                # None
print(hasattr(e, 'name'))   # True
print(hasattr(e, 'obj'))    # True

# AttributeError subclass inherits name and obj
class MyAttrError(AttributeError):
    pass

e2 = MyAttrError('sub')
print(e2.name)              # None
print(e2.obj)               # None

# Module attribute miss: name is set
import sys
try:
    sys.no_such_module_attr
except AttributeError as e:
    print(e.name)           # no_such_module_attr

# hasattr returns False when attribute is absent (not leaking the error)
class Foo:
    pass

obj = Foo()
print(hasattr(obj, 'missing'))  # False
