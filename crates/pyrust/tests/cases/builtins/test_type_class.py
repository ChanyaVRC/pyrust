"""Parity fixture for issue #1190.

`type` must be a proper PyClass singleton (not a BuiltinFunction), so that:
  - isinstance(type, type) is True
  - issubclass(type, object) is True
  - type.__name__ == 'type'
  - type.__bases__ == (object,)
  - type(x) still returns the class of x
  - isinstance(int, type) is True (all classes are instances of type)
  - class Meta(type): pass does not raise RuntimeError
"""

# type resolves to <class 'type'>, not <built-in function type>
print(type)                          # <class 'type'>

# Metaclass checks: type is its own type
print(isinstance(type, type))        # True
print(issubclass(type, object))      # True
print(type.__name__)                 # type
print(type.__bases__)                # (<class 'object'>,)
print(type(type) is type)            # True

# 1-arg type() must still work
print(type(42))                      # <class 'int'>
print(type("hello"))                 # <class 'str'>
print(type(42).__name__)             # int

# All primitive class objects are instances of type
print(isinstance(int, type))         # True
print(isinstance(str, type))         # True
print(isinstance(list, type))        # True

# Instances of classes are not instances of type
print(isinstance(42, type))          # False
print(isinstance("hello", type))     # False

# User-defined classes are also instances of type
class Foo:
    pass

print(isinstance(Foo, type))         # True
print(isinstance(Foo(), type))       # False

# Subclassing type (metaclass usage) must not raise RuntimeError
class Meta(type):
    pass

print(isinstance(Meta, type))        # True
print(issubclass(Meta, type))        # True

# metaclass= keyword must work with a type subclass
class Bar(metaclass=Meta):
    x = 10

print(isinstance(Bar, Meta))         # True
print(isinstance(Bar, type))         # True
print(Bar.x)                         # 10
