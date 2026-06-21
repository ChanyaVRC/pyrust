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

# Subscripting a user generic yields a module-qualified alias with the subclass
# as origin, exposes __origin__ / __args__, and is callable (constructs the
# origin, dropping the type args) — matching CPython's _GenericAlias.
alias = Stack[int]
print(alias)
print(alias.__origin__ is Stack)
print(alias.__args__)
constructed = Stack[int]()
print(type(constructed).__name__)
print(constructed.items)
