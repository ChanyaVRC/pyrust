# Parity fixture for issue #1626: type(Foo) should return the metaclass when
# the class was created with metaclass=Meta, not always the built-in `type`.

class Meta(type):
    pass

class Foo(metaclass=Meta):
    pass

# Basic: type(Foo) is the metaclass
print(type(Foo).__name__)        # Meta
print(type(Foo) is Meta)         # True

# isinstance: Foo is an instance of Meta (Meta is its metatype)
print(isinstance(Foo, Meta))     # True
print(isinstance(Foo, type))     # True (Meta is a subclass of type)
print(isinstance(Foo, object))   # True (universal)

# Metaclass itself uses the built-in `type` as its metatype
print(type(Meta).__name__)       # type
print(type(Meta) is type)        # True

# Class instances are unaffected
foo = Foo()
print(isinstance(foo, Foo))      # True
print(type(foo).__name__)        # Foo

# Unrelated classes retain the plain type metatype
print(type(int).__name__)        # type
print(type(int) is type)         # True

# Metaclass inheritance: classes that do NOT specify metaclass= stay plain type
class Bar:
    pass

print(type(Bar).__name__)        # type
print(type(Bar) is type)         # True

# Multi-level metaclass
class MetaMeta(type):
    pass

class Meta2(type, metaclass=MetaMeta):
    pass

class Baz(metaclass=Meta2):
    pass

print(type(Baz).__name__)        # Meta2
print(isinstance(Baz, Meta2))    # True
print(isinstance(Baz, type))     # True
print(type(Meta2).__name__)      # MetaMeta
print(isinstance(Meta2, MetaMeta))  # True
