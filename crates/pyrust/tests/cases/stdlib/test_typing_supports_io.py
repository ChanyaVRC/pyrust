# Pins CPython 3.12.3's `typing.Supports*` runtime-checkable protocols, the
# generic `IO` / `TextIO` / `BinaryIO` classes, and the `Generic` subclass
# `__parameters__` bookkeeping they rely on (issue #3326).
import decimal
import fractions
import typing
from typing import (
    IO, BinaryIO, Generic, Iterable, Mapping, Protocol, SupportsAbs,
    SupportsBytes, SupportsComplex, SupportsFloat, SupportsIndex, SupportsInt,
    SupportsRound, TextIO, TypeVar, AnyStr, runtime_checkable,
)

T = TypeVar("T")
K = TypeVar("K")


def show(label, thunk):
    try:
        print(label, "->", repr(thunk()))
    except Exception as exc:  # noqa: BLE001 - the error text is the point
        print(label, "-> raises", type(exc).__name__, exc)


PROTOCOLS = [
    SupportsInt, SupportsFloat, SupportsComplex, SupportsBytes,
    SupportsIndex, SupportsAbs, SupportsRound,
]

print("== protocol classes ==")
for proto in PROTOCOLS:
    print(
        repr(proto), proto.__module__, proto.__qualname__, proto.__mro__,
        sorted(proto.__protocol_attrs__), proto.__parameters__,
        sorted(proto.__abstractmethods__), proto.__doc__,
    )
print(SupportsAbs.__type_params__, SupportsRound.__type_params__)
print(SupportsAbs.__orig_bases__)
print(SupportsAbs[int], SupportsRound[float].__args__, typing.get_origin(SupportsAbs[int]))
show("SupportsInt[int]", lambda: SupportsInt[int])
show("SupportsAbs[int, str]", lambda: SupportsAbs[int, str])

print("== isinstance / issubclass on builtins ==")
VALUES = [
    1, True, 1.5, 1j, "s", b"b", bytearray(b"x"), None, [],
    fractions.Fraction(1, 2), decimal.Decimal("1.5"), memoryview(b"x"),
]
for proto in PROTOCOLS:
    print(
        proto.__name__,
        [isinstance(value, proto) for value in VALUES],
        [issubclass(type(value), proto) for value in VALUES],
    )


class Numberish:
    def __int__(self):
        return 3

    def __float__(self):
        return 3.0

    def __index__(self):
        return 3

    def __abs__(self):
        return 3

    def __round__(self, ndigits=0):
        return 3

    def __complex__(self):
        return 3j

    def __bytes__(self):
        return b"3"


class OnlyInt:
    def __int__(self):
        return 1


class NotInt:
    __int__ = None


print("== structural checks on user classes ==")
for proto in PROTOCOLS:
    print(
        proto.__name__, isinstance(Numberish(), proto), issubclass(Numberish, proto),
        isinstance(OnlyInt(), proto), isinstance(object(), proto),
    )
print(isinstance(NotInt(), SupportsInt), issubclass(NotInt, SupportsInt))


class Explicit(SupportsInt):
    def __int__(self):
        return 7


print(int(Explicit()), Explicit.__mro__, sorted(Explicit.__abstractmethods__))
print(isinstance(Explicit(), SupportsInt), issubclass(Explicit, SupportsInt))

print("== numeric conversions used by the protocols ==")
print((1.5).__int__(), (2.5).__float__(), (1j).__complex__(), b"x".__bytes__())
print(float.__int__(-2.7), complex.__complex__(2 + 3j), bytes.__bytes__(b"q"))

print("== IO classes ==")
print(IO, TextIO, BinaryIO, IO.__module__, TextIO.__qualname__)
print(IO.__mro__, TextIO.__mro__, BinaryIO.__mro__)
print(IO.__parameters__, TextIO.__parameters__, BinaryIO.__parameters__)
print(IO.__orig_bases__, TextIO.__orig_bases__, BinaryIO.__orig_bases__)
print(IO[str], IO[bytes], IO[AnyStr], typing.get_origin(IO[str]), typing.get_args(IO[bytes]))
print(sorted(name for name in vars(IO) if not name.startswith("__")))
print(sorted(name for name in vars(TextIO) if not name.startswith("__")))
print(sorted(name for name in vars(BinaryIO) if not name.startswith("__")))
print(IO.read.__isabstractmethod__, TextIO.__enter__.__isabstractmethod__)
print(IO.__slots__, TextIO.__slots__, BinaryIO.__slots__)
print(IO.readlines.__annotations__, BinaryIO.write.__annotations__)
print(TextIO.errors.fget.__annotations__, TextIO.newlines.fget.__annotations__)
show("IO[int, str]", lambda: IO[int, str])
show("TextIO[str]", lambda: TextIO[str])

print("== Generic __parameters__ ==")


class Box(Generic[T]):
    pass


class Pair(Generic[K, T]):
    pass


class IntBox(Box[int]):
    pass


class Implicit[X, Y]:
    pass


class Derived(Box[T], Iterable[K]):
    def __iter__(self):
        return iter(())


print(Box.__parameters__, Pair.__parameters__, IntBox.__parameters__)
print(Implicit.__parameters__, Derived.__parameters__, Derived.__mro__)
print(Box[int] == Box[int], Box[int] == Box[str], hash(Box[int]) == hash(Box[int]))
print(Box[T][int], Pair[K, T][str, int], Pair[K, Box[T]].__parameters__)
print(Box[int] | None, Box[T].__parameters__, Box[int].__name__)
print(Box[int]().__orig_class__, type(Box[int]()).__name__)
show("isinstance(Box(), Box[int])", lambda: isinstance(Box(), Box[int]))
show("issubclass(Box, Box[int])", lambda: issubclass(Box, Box[int]))
show("Box[int, str]", lambda: Box[int, str])
show("Pair[int]", lambda: Pair[int])
show("IntBox[int]", lambda: IntBox[int])
show("Box[int][str]", lambda: Box[int][str])
show("Generic[T][int]", lambda: Generic[T][int])


def plain_generic():
    class Bad(Generic):
        pass
    return Bad


def missing_var():
    class Bad(Box[T], Generic[K]):
        pass
    return Bad


def double_generic():
    class Bad(Generic[T], Generic[K]):
        pass
    return Bad


show("class Bad(Generic)", plain_generic)
show("class Bad(Box[T], Generic[K])", missing_var)
show("class Bad(Generic[T], Generic[K])", double_generic)


@runtime_checkable
class Sized2(Protocol[T]):
    def __len__(self) -> int: ...


print(Sized2.__parameters__, sorted(Sized2.__protocol_attrs__), isinstance([], Sized2))
print(Sized2[int], Mapping[str, Sized2[int]])
