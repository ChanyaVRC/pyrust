"""issubclass against a @runtime_checkable typing.Protocol (issue #2552).

``issubclass(C, P)`` against a runtime-checkable non-data Protocol structurally
checks that ``C``'s MRO defines every protocol member.  Protocols carrying any
non-method (data) member raise ``TypeError`` for ``issubclass`` while still
permitting ``isinstance``, matching CPython 3.12.
"""

from typing import Protocol, runtime_checkable


@runtime_checkable
class Sized(Protocol):
    def __len__(self): ...


class MyCollection:
    def __len__(self):
        return 0


class NoLen:
    pass


# Class-side structural check across the candidate's MRO.
print(issubclass(MyCollection, Sized))  # True
print(issubclass(NoLen, Sized))  # False

# Built-in types participate via their type MRO.
print(issubclass(list, Sized))  # True
print(issubclass(dict, Sized))  # True
print(issubclass(int, Sized))  # False


# Multi-method protocol: the class must provide ALL members.
@runtime_checkable
class RW(Protocol):
    def read(self): ...

    def write(self): ...


class OnlyRead:
    def read(self):
        return 1


class Both:
    def read(self):
        return 1

    def write(self):
        return 2


print(issubclass(OnlyRead, RW))  # False
print(issubclass(Both, RW))  # True


# Inherited members count: a base class supplying a method satisfies the protocol.
class Base:
    def read(self):
        return 1


class Derived(Base):
    def write(self):
        return 2


print(issubclass(Derived, RW))  # True


# Protocol inheritance: requirements accumulate across the protocol's own MRO.
@runtime_checkable
class A(Protocol):
    def a(self): ...


@runtime_checkable
class B(A, Protocol):
    def b(self): ...


class ImplBoth:
    def a(self): ...

    def b(self): ...


class ImplOne:
    def b(self): ...


print(issubclass(ImplBoth, B))  # True
print(issubclass(ImplOne, B))  # False


# Data-member protocol: issubclass raises TypeError, isinstance is allowed.
@runtime_checkable
class HasName(Protocol):
    name: str


class WithName:
    name = "x"


print(isinstance(WithName(), HasName))  # True
try:
    issubclass(WithName, HasName)
    print("NO ERROR")
except TypeError as e:
    print("TypeError:", e)


# An empty protocol imposes no requirements: every class is a subclass.
@runtime_checkable
class Empty(Protocol):
    pass


print(issubclass(int, Empty))  # True
print(issubclass(MyCollection, Empty))  # True


# A non-runtime-checkable Protocol rejects issubclass.
class NotRC(Protocol):
    def foo(self): ...


try:
    issubclass(object, NotRC)
    print("NO ERROR")
except TypeError as e:
    print("TypeError:", e)


# arg 1 must be a class: a non-class first argument raises before the protocol's
# own data-member TypeError (CPython error precedence).
try:
    issubclass(42, HasName)
    print("NO ERROR")
except TypeError as e:
    print("TypeError:", e)


# Tuple classinfo with a protocol leaf: matches if any leaf matches.
print(issubclass(list, (int, Sized)))  # True via Sized
print(issubclass(int, (str, Sized)))  # False
