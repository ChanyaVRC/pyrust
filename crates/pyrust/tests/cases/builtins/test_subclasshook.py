# Issue #1738: object.__subclasshook__ should exist and return NotImplemented.
# CPython 3.12: object.__subclasshook__ is a classmethod_descriptor that
# always returns NotImplemented, used by ABCMeta.__subclasscheck__ to
# allow custom issubclass() behaviour.

# Basic: object.__subclasshook__ exists.
print(hasattr(object, '__subclasshook__'))

# Calling it returns NotImplemented.
print(object.__subclasshook__(int))
print(object.__subclasshook__(str))

# Subclasses inherit it.
class Foo:
    pass

print(Foo.__subclasshook__(int))
print(Foo.__subclasshook__(Foo))

# A class that defines its own __subclasshook__ overrides the default.
class Bar:
    @classmethod
    def __subclasshook__(cls, subclass):
        if subclass is int:
            return True
        return NotImplemented

print(Bar.__subclasshook__(int))
print(Bar.__subclasshook__(str))

# Keyword args are rejected even though positional args are accepted freely.
try:
    object.__subclasshook__(subclass=int)
except TypeError as e:
    print(type(e).__name__, e)

# Any number of positional args is accepted (CPython ignores them all).
print(object.__subclasshook__())
print(object.__subclasshook__(int, str, float))
