"""Parity tests for `type` as the metaclass (issue #1312).

`type` is both a callable and a class in CPython.  These tests verify that:
  - bare `type` and `builtins.type` display as `<class 'type'>`
  - `type(x)` still works as a 1-arg callable
  - `type(name, bases, dict)` still works as a 3-arg class factory
  - `isinstance(int, type)` and similar metaclass checks work correctly
  - `type is type` and `builtins.type is type` hold (singleton identity)
"""

import builtins

# builtins.type must display as `<class 'type'>`, not `<built-in function type>`
print(repr(builtins.type))

# type(int) / type(str) must also display as `<class 'type'>`
print(repr(type(int)))
print(repr(type(str)))
print(repr(type(list)))

# isinstance metaclass checks
print(isinstance(int, type))     # True: int is a class
print(isinstance(str, type))     # True: str is a class
print(isinstance(42, type))      # False: 42 is an int instance, not a class
print(isinstance("hi", type))    # False

# isinstance with user-defined classes
class Foo:
    pass

print(isinstance(Foo, type))     # True: Foo is a class
print(isinstance(Foo(), type))   # False: Foo() is an instance, not a class

# type is type (identity)
print(type is type)              # True
print(builtins.type is type)     # True

# 1-arg type() still works
print(type(42))                  # <class 'int'>
print(type("hello"))             # <class 'str'>
print(type([]))                  # <class 'list'>
print(type(True))                # <class 'bool'>

# 3-arg type() class factory still works
MyClass = type("MyClass", (object,), {"x": 42})
print(MyClass.x)                 # 42
print(isinstance(MyClass, type)) # True: dynamically created class is still a class

# type(type) is type
print(type(type) is type)        # True
