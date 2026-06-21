from abc import ABC, abstractmethod


class Drawable(ABC):
    @classmethod
    def __subclasshook__(cls, C):
        if cls is Drawable:
            if hasattr(C, 'draw'):
                return True
        return NotImplemented


class Circle:  # doesn't inherit Drawable
    def draw(self):
        return "circle"


class Square:  # also doesn't inherit
    pass


print(isinstance(Circle(), Drawable))    # True
print(issubclass(Circle, Drawable))      # True
print(isinstance(Square(), Drawable))    # False
print(issubclass(Square, Drawable))      # False


# Normal inheritance still works
class Triangle(Drawable):
    def area(self):
        return 1

    def draw(self):
        return "triangle"


print(isinstance(Triangle(), Drawable))  # True


# Hook returning NotImplemented falls through to MRO
class Fallback(ABC):
    @classmethod
    def __subclasshook__(cls, C):
        return NotImplemented


print(issubclass(Fallback, Fallback))    # True (normal MRO)


# A hook returning False is authoritative even for a real subclass.
class Strict(ABC):
    @classmethod
    def __subclasshook__(cls, C):
        return False


class StrictChild(Strict):
    pass


print(issubclass(StrictChild, Strict))   # False (hook overrides MRO)


# An ABC without a custom hook still resolves normally via object.__subclasshook__.
class Plain(ABC):
    @abstractmethod
    def go(self):
        ...


class PlainChild(Plain):
    def go(self):
        return 1


print(issubclass(PlainChild, Plain))     # True
print(isinstance(PlainChild(), Plain))   # True
print(issubclass(int, Plain))            # False


# collections.abc still works (no regression)
from collections.abc import Iterable
print(isinstance([1, 2, 3], Iterable))   # True
print(isinstance(42, Iterable))          # False


# A hook must return exactly bool or NotImplemented; a non-bool result raises
# AssertionError, matching CPython's _abc_subclasscheck contract.
class BadHook(ABC):
    @classmethod
    def __subclasshook__(cls, C):
        return 1


class Probe:
    pass


try:
    issubclass(Probe, BadHook)
except AssertionError as e:
    print("AssertionError:", e)
