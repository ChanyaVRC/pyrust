# Generic.__class_getitem__ direct call and subscript parity (issue #2707).
from typing import TypeVar, Generic

T = TypeVar("T")
U = TypeVar("U")

# Direct method-call form returns a _GenericAlias, not the bare class.
result = Generic.__class_getitem__(T)
print(result)
print(type(result).__name__)

# Subscript form must match the direct-call form.
sub = Generic[T]
print(sub)
print(type(sub).__name__)
print(repr(result) == repr(sub))

# Multiple type parameters.
print(Generic.__class_getitem__((T, U)))
print(Generic[T, U])

# TypeVar repr carries the variance prefix.
print(repr(T))
print(repr(TypeVar("T_co", covariant=True)))
print(repr(TypeVar("T_contra", contravariant=True)))

# Class-base path stays unbroken.
class Stack(Generic[T]):
    def __init__(self):
        self.items = []

    def push(self, x):
        self.items.append(x)


s = Stack()
s.push(1)
s.push(2)
print(s.items)
print(Stack.__name__)
print(issubclass(Stack, Generic))
