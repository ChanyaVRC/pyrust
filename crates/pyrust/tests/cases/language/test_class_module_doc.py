# Parity fixture for issue #1275: __module__ and __doc__ on built-in types
# and exception classes.

# Primitive types: __module__ must be 'builtins'
print(int.__module__)         # builtins
print(str.__module__)         # builtins
print(list.__module__)        # builtins
print(dict.__module__)        # builtins
print(tuple.__module__)       # builtins
print(set.__module__)         # builtins
print(bytes.__module__)       # builtins
print(float.__module__)       # builtins
print(bool.__module__)        # builtins

# Exception classes: __module__ must be 'builtins'
print(ValueError.__module__)  # builtins
print(TypeError.__module__)   # builtins
print(Exception.__module__)   # builtins
print(BaseException.__module__)  # builtins

# object: __module__ must be 'builtins'
print(object.__module__)      # builtins

# __doc__ on primitive types must be a non-None string
print(isinstance(int.__doc__, str))   # True
print(isinstance(str.__doc__, str))   # True
print(isinstance(list.__doc__, str))  # True
print(isinstance(float.__doc__, str)) # True
print(isinstance(bool.__doc__, str))  # True

# Spot-check exact first line of selected docstrings (catches truncation bugs)
print(int.__doc__.split('\n')[0])     # int([x]) -> integer
print(str.__doc__.split('\n')[0])     # str(object='') -> str
print(dict.__doc__.split('\n')[0])    # dict() -> new empty dictionary
print(tuple.__doc__.split('\n')[0])   # Built-in immutable sequence.

# __doc__ on exception classes must be a non-None string
print(isinstance(ValueError.__doc__, str))   # True
print(isinstance(TypeError.__doc__, str))    # True
print(isinstance(Exception.__doc__, str))    # True

# User-defined classes are unaffected: __module__ == '__main__'
class Foo:
    pass
print(Foo.__module__)   # __main__
print(Foo.__doc__)      # None

class Bar:
    """My docstring"""
    pass
print(Bar.__module__)   # __main__
print(Bar.__doc__)      # My docstring

# Subclass of a builtin exception gets __module__ = '__main__'
class MyError(ValueError):
    pass
print(MyError.__module__)  # __main__
