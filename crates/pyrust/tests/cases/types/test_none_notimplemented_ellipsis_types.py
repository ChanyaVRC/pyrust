# type() for None, NotImplemented, and Ellipsis must return a proper class object
# (not a BuiltinFunction sentinel), matching CPython 3.12 behaviour.

print(type(None))
print(type(NotImplemented))
print(type(...))

print(type(None).__name__)
print(type(NotImplemented).__name__)
print(type(...).__name__)

# The class objects are singletons: type(None) is type(None).
print(type(None) is type(None))
print(type(NotImplemented) is type(NotImplemented))
print(type(...) is type(...))

# type(type(None)) is <class 'type'> — the metaclass.
print(type(type(None)))
print(type(type(...)))

# isinstance
print(isinstance(None, type(None)))
print(isinstance(..., type(...)))
print(isinstance(NotImplemented, type(NotImplemented)))

# negative isinstance
print(isinstance(None, type(...)))
print(isinstance(..., type(None)))
print(isinstance(42, type(None)))

# isinstance with tuple
print(isinstance(None, (type(None), int)))
print(isinstance(42, (type(None), int)))

# issubclass — every type is a subclass of itself and of object
print(issubclass(type(None), type(None)))
print(issubclass(type(None), object))
print(issubclass(type(...), object))
print(issubclass(type(NotImplemented), object))

# issubclass negative
print(issubclass(type(None), type(...)))
print(issubclass(type(None), int))
