# Issue #2698: subscripting a class calls __class_getitem__ (inherited via the
# MRO too), so `Stack[int]` for `class Stack(Generic[T])` yields a generic alias
# instead of raising "type 'Stack' is not subscriptable".
from typing import TypeVar, Generic, Protocol

T = TypeVar('T')
S = TypeVar('S')


class Stack(Generic[T]):
    pass


# Generic subclass subscript -> generic alias (CPython: typing._GenericAlias).
result = Stack[int]
print('GenericAlias' in type(result).__name__)
print(str(result))
print(result.__origin__ is Stack)
print(result.__args__)

# Multi-parameter generic.
class Pair(Generic[T, S]):
    pass


print(str(Pair[int, str]))

# Protocol subclass subscript.
class Proto(Protocol[T]):
    pass


print(str(Proto[int]))

# Constructing an instance through the alias yields the origin class instance.
print(type(Stack[int]()).__name__)

# User-defined __class_getitem__ on the class itself.
class Foo:
    def __class_getitem__(cls, item):
        return f"Foo[{item.__name__}]"


print(Foo[int])
print(Foo[str])

# Inherited user-defined __class_getitem__ (resolved via the MRO).
class Base:
    def __class_getitem__(cls, item):
        return f"Base[{item.__name__}]"


class Sub(Base):
    pass


print(Sub[int])

# A class with no __class_getitem__ anywhere in its MRO stays non-subscriptable.
class Bar:
    pass


try:
    Bar[int]
except TypeError:
    print("TypeError: Bar not subscriptable")
