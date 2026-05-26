"""
Parity fixture for issue #1313: `object` must be registered in the builtins module.
"""
import builtins

# builtins.object is accessible and is the same object as the bare name
print(builtins.object)
print(hasattr(builtins, 'object'))
print(builtins.object is object)

# __name__ and __module__ on the object class
print(object.__name__)
print(object.__module__)

# isinstance: every value is an instance of object
print(isinstance(42, object))
print(isinstance('hi', object))
print(isinstance(None, object))
print(isinstance(True, object))
print(isinstance([], object))
print(isinstance({}, object))
print(isinstance((), object))
print(isinstance(set(), object))
print(isinstance(b'', object))
print(isinstance(1.0, object))
print(isinstance(1 + 2j, object))

# issubclass: all primitive types are subclasses of object
print(issubclass(int, object))
print(issubclass(str, object))
print(issubclass(list, object))
print(issubclass(tuple, object))
print(issubclass(dict, object))
print(issubclass(set, object))
print(issubclass(bytes, object))
print(issubclass(float, object))
print(issubclass(complex, object))
print(issubclass(bool, object))
print(issubclass(frozenset, object))
print(issubclass(object, object))

# User-defined classes inherit from object implicitly
class Foo:
    pass

print(issubclass(Foo, object))
foo = Foo()
print(isinstance(foo, object))
print(Foo.__bases__)
