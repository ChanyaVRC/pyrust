"""
Parity fixture for issue #1313: object and type must be accessible via builtins module.

CPython 3.12:
  import builtins; builtins.object   # <class 'object'>
  import builtins; builtins.type     # <class 'type'>
  import builtins; builtins.int is int  # True
"""
import builtins

# builtins.object is accessible and identical to bare 'object'
print(builtins.object)
print(hasattr(builtins, 'object'))
print(builtins.object is object)

# builtins.type is accessible and identical to bare 'type'
print(builtins.type)
print(hasattr(builtins, 'type'))
print(builtins.type is type)

# All primitive types accessible via builtins and identical to bare names
print(builtins.int is int)
print(builtins.str is str)
print(builtins.list is list)
print(builtins.tuple is tuple)
print(builtins.dict is dict)
print(builtins.set is set)
print(builtins.frozenset is frozenset)
print(builtins.bytes is bytes)
print(builtins.float is float)
print(builtins.complex is complex)
print(builtins.bool is bool)

# object.__name__ and object.__module__
print(object.__name__)
print(object.__module__)

# isinstance with builtins.object — every value is an instance of object
print(isinstance(42, builtins.object))
print(isinstance('hi', builtins.object))
print(isinstance(None, builtins.object))
print(isinstance(True, builtins.object))
print(isinstance([], builtins.object))
print(isinstance({}, builtins.object))

# issubclass with builtins.object
print(issubclass(int, builtins.object))
print(issubclass(str, builtins.object))
print(issubclass(object, builtins.object))

# User-defined class inherits from object
class Foo:
    pass

print(isinstance(Foo(), builtins.object))
print(issubclass(Foo, builtins.object))
print(Foo.__bases__)
