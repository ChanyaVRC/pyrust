"""Structural isinstance for @runtime_checkable typing.Protocol (issue #2526)."""

from typing import Protocol, runtime_checkable


@runtime_checkable
class Sized(Protocol):
    def __len__(self): ...


# Subjects with __len__ structurally satisfy Sized; ints do not.
print(isinstance([1, 2], Sized))
print(isinstance("hello", Sized))
print(isinstance({}, Sized))
print(isinstance(42, Sized))


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


# All required methods must be present.
print(isinstance(OnlyRead(), RW))
print(isinstance(Both(), RW))


@runtime_checkable
class HasName(Protocol):
    name: str


class WithName:
    name = "x"


class WithoutName:
    pass


# Data-member protocols match on attribute presence.
print(isinstance(WithName(), HasName))
print(isinstance(WithoutName(), HasName))


@runtime_checkable
class A(Protocol):
    def foo(self): ...


@runtime_checkable
class B(A, Protocol):
    def bar(self): ...


class HasBoth:
    def foo(self): ...

    def bar(self): ...


class OnlyBar:
    def bar(self): ...


# Inherited protocol requirements are included.
print(isinstance(HasBoth(), B))
print(isinstance(OnlyBar(), B))


@runtime_checkable
class Empty(Protocol):
    pass


# An empty protocol body has no requirements: everything matches.
print(isinstance(42, Empty))


class NotRC(Protocol):
    def foo(self): ...


# A Protocol subclass without @runtime_checkable rejects instance checks.
try:
    isinstance(object(), NotRC)
    print("NO ERROR")
except TypeError as e:
    print("TypeError:", e)
