# Issue #1955: isinstance()/issubclass() must consult a metaclass's
# __instancecheck__ / __subclasscheck__ override (CPython dispatches these via
# type(cls), the metaclass — not the class's own dict).

class Meta(type):
    def __instancecheck__(cls, inst):
        return inst == 42

    def __subclasscheck__(cls, sub):
        return sub is int


class V(metaclass=Meta):
    pass


# __instancecheck__ override controls isinstance().
print(isinstance(42, V))   # True
print(isinstance(5, V))    # False

# __subclasscheck__ override controls issubclass().
print(issubclass(int, V))  # True
print(issubclass(str, V))  # False

# Ordinary classes (metaclass is `type`) keep the normal MRO-based check.
class Animal:
    pass


class Dog(Animal):
    pass


print(isinstance(Dog(), Animal))   # True
print(isinstance(Dog(), Dog))      # True
print(isinstance(42, Animal))      # False
print(issubclass(Dog, Animal))     # True
print(issubclass(Animal, Dog))     # False

# Tuple / union forms still work for ordinary classes.
print(isinstance(5, (str, int)))   # True
print(issubclass(bool, int))       # True

# Built-in ABC __instancecheck__ path is unaffected.
from collections.abc import Iterable

print(isinstance([1, 2], Iterable))  # True
print(issubclass(list, Iterable))    # True
