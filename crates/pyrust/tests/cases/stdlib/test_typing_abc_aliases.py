# Pins the CPython 3.12.3 `typing` aliases for `collections.abc`, `contextlib`,
# `collections`, and `re` (issue #3326): `_SpecialGenericAlias` values and the
# named `_GenericAlias` produced by subscripting them, plus `Text`, `AnyStr`,
# `ForwardRef`, and `typing.__all__`.
import collections
import collections.abc as cabc
import contextlib
import copy
import re
import typing
import warnings
from typing import (
    Iterable, Sequence, MutableSequence, Mapping, MutableMapping,
    Iterator, Generator, Awaitable, Coroutine, AsyncIterator, AsyncIterable,
    AsyncGenerator, AbstractSet, MutableSet, Collection, Container, Sized,
    Hashable, Reversible, KeysView, ValuesView, ItemsView, MappingView,
    ContextManager, AsyncContextManager, Counter, DefaultDict, Deque,
    OrderedDict, ChainMap, ByteString, Text, AnyStr, IO, TextIO, BinaryIO,
    Pattern, Match, SupportsInt, SupportsFloat, SupportsAbs, SupportsBytes,
    SupportsComplex, SupportsIndex, SupportsRound,
    Any, Callable, ClassVar, Dict, Final, ForwardRef, Generic, List,
    Optional, Protocol, Tuple, Type, TypeVar, Union, get_args, get_origin,
)

T = TypeVar("T")
K = TypeVar("K")
V = TypeVar("V")


def show(label, thunk):
    try:
        print(label, "->", repr(thunk()))
    except Exception as exc:  # noqa: BLE001 - the error text is the point
        print(label, "-> raises", type(exc).__name__, exc)


print("== issue reproducer ==")
print(Iterable[int], Mapping[str, int], Sequence[str])

print("== __all__ ==")
print(len(typing.__all__), len(set(typing.__all__)))
print(sorted(typing.__all__))
print([name for name in typing.__all__ if not hasattr(typing, name)])
namespace = {}
exec("from typing import *", namespace)
print(sorted(name for name in namespace if name != "__builtins__") == sorted(typing.__all__))

print("== bare aliases ==")
ALIASES = [
    "Hashable", "Awaitable", "Coroutine", "AsyncIterable", "AsyncIterator",
    "Iterable", "Iterator", "Reversible", "Sized", "Container", "Collection",
    "AbstractSet", "MutableSet", "Mapping", "MutableMapping", "Sequence",
    "MutableSequence", "ByteString", "Deque", "MappingView", "KeysView",
    "ItemsView", "ValuesView", "ContextManager", "AsyncContextManager",
    "DefaultDict", "OrderedDict", "Counter", "ChainMap", "Generator",
    "AsyncGenerator", "Pattern", "Match",
]
for name in ALIASES:
    alias = getattr(typing, name)
    print(
        name, repr(alias), type(alias).__name__, alias.__origin__,
        alias._nparams, alias.__name__, alias.__qualname__, alias.__module__,
        get_origin(alias), get_args(alias), alias.__doc__,
    )
print(AbstractSet.__origin__ is cabc.Set, Deque.__origin__ is collections.deque)
print(ContextManager.__origin__ is contextlib.AbstractContextManager)
print(Pattern.__origin__ is re.Pattern, Match.__origin__ is re.Match)
print(typing.Iterable is typing.Iterable, Iterable is not cabc.Iterable)

print("== subscription repr / origin / args ==")
SUBSCRIPTS = [
    "Iterable[int]", "Sequence[str]", "MutableSequence[bytes]",
    "Mapping[str, int]", "MutableMapping[str, list]",
    "Iterator[Tuple[int, str]]", "Generator[int, str, bool]",
    "Awaitable[int]", "Coroutine[int, str, bool]", "AsyncIterator[int]",
    "AsyncIterable[int]", "AsyncGenerator[int, None]", "AbstractSet[int]",
    "MutableSet[int]", "Collection[int]", "Container[int]", "Reversible[int]",
    "KeysView[str]", "ValuesView[int]", "ItemsView[str, int]",
    "MappingView[int]", "ContextManager[int]", "AsyncContextManager[int]",
    "Counter[str]", "DefaultDict[str, int]", "Deque[int]",
    "OrderedDict[str, int]", "ChainMap[str, int]", "Pattern[str]",
    "Match[bytes]", "IO[str]", "Iterable[None]", "Iterable['int']",
    "Iterable[...]", "Iterable[Any]", "Iterable[Callable]",
    "Iterable[Iterable]", "Sequence[List[int]]", "Dict[str, Iterable[int]]",
    "Type[Iterable[int]]", "list[Iterable[int]]",
    "Callable[[Iterable[int]], Sequence[str]]", "Tuple[Iterable[int], ...]",
    "Mapping[str, T]", "Iterable[T]",
]
for expr in SUBSCRIPTS:
    alias = eval(expr)
    print(expr, "|", repr(alias), "|", get_origin(alias), "|", get_args(alias))

print("== metadata of a subscripted alias ==")
alias = Mapping[str, int]
print(type(alias).__name__, alias.__origin__, alias.__args__, alias.__parameters__)
print(alias.__name__, alias.__qualname__, alias.__module__, alias._name)
print(Mapping[K, List[V]].__parameters__, Iterable[T].__parameters__)
print(Mapping.keys is cabc.Mapping.keys, Mapping[str, int].get is cabc.Mapping.get)

print("== arity and parameter checks ==")
for expr in [
    "Hashable[int]", "Sized[int]", "ByteString[int]", "Mapping[int]",
    "Iterable[int, str]", "Generator[int]", "Coroutine[int, str]",
    "ItemsView[int]", "Iterable[(int, str)]", "Mapping[(int, str), int]",
    "Iterable[Union]", "Iterable[Optional]", "Iterable[ClassVar]",
    "Iterable[Generic]", "Iterable[Protocol]", "Iterable[ClassVar[int]]",
    "Iterable[Final[int]]", "Iterable[Generic[T]]", "Iterable[int][str]",
    "Mapping[str, T][int, str]",
]:
    show(expr, lambda: eval(expr))

print("== type-variable substitution ==")
for expr in [
    "Mapping[K, V][str, int]", "Iterable[T][int]", "Mapping[str, T][int]",
    "Mapping[str, List[T]][int]", "Mapping[str, Iterable[T]][bytes]",
    "Iterable[Optional[T]][int]", "Iterable[Callable[[T], int]][str]",
    "Iterable[T][None]", "Iterable[T]['int']",
]:
    show(expr, lambda: eval(expr))

print("== equality / hashing ==")
print(Iterable[int] == Iterable[int], Iterable[int] != Iterable[str])
print(Iterable[int] == cabc.Iterable[int], Iterable[int] == Iterable)
print(Mapping[str, int] == Mapping[str, int], Mapping[str, int] == MutableMapping[str, int])
print(hash(Iterable[int]) == hash(Iterable[int]))
print({Iterable[int]: "hit"}[Iterable[int]])
print(len({Iterable[int], Iterable[int], Iterable[str]}))

print("== unions ==")
for expr in [
    "Optional[Iterable[int]]", "Union[Iterable[int], None]",
    "Union[Iterable[int], Iterable[int]]", "Union[Iterable, Iterable]",
    "Union[Iterable[int], Iterable[str]]", "Iterable[int] | None",
    "None | Iterable[int]", "int | Iterable[int]", "Iterable | None",
    "Iterable | Mapping", "get_args(Optional[Iterable[int]])",
    "get_origin(Iterable[int] | None) is Union",
]:
    show(expr, lambda: eval(expr))

print("== isinstance / issubclass ==")
CHECKS = [
    "isinstance([], Iterable)", "isinstance(1, Iterable)",
    "isinstance({}, Mapping)", "isinstance({}, MutableMapping)",
    "isinstance([], Sequence)", "isinstance('a', Sequence)",
    "isinstance((x for x in []), Generator)", "isinstance(iter([]), Iterator)",
    "isinstance(1, Hashable)", "isinstance([], Hashable)",
    "isinstance([], Sized)", "isinstance(set(), AbstractSet)",
    "isinstance(frozenset(), MutableSet)", "isinstance({}.keys(), KeysView)",
    "isinstance({}.items(), ItemsView)", "isinstance({}.values(), ValuesView)",
    "isinstance(collections.Counter(), Counter)", "isinstance({}, Counter)",
    "isinstance(collections.deque(), Deque)",
    "isinstance(collections.defaultdict(), DefaultDict)",
    "isinstance(collections.OrderedDict(), OrderedDict)",
    "isinstance(collections.ChainMap(), ChainMap)",
    "isinstance(re.compile('a'), Pattern)",
    "isinstance(re.match('a', 'a'), Match)",
    "isinstance(contextlib.nullcontext(), ContextManager)",
    "isinstance([], (int, Iterable))", "isinstance([], Union[Iterable, int])",
    "issubclass(list, Iterable)", "issubclass(dict, Mapping)",
    "issubclass(int, Iterable)", "issubclass(list, MutableSequence)",
    "issubclass(tuple, MutableSequence)", "issubclass(Sequence, Iterable)",
    "issubclass(Iterable, Sequence)", "issubclass(str, Container)",
    "issubclass(bytes, ByteString)", "issubclass(list, Reversible)",
    "isinstance([], Iterable[int])", "issubclass(list, Iterable[int])",
    "issubclass(Iterable[int], Iterable)", "isinstance([], Mapping[str, int])",
]
for expr in CHECKS:
    show(expr, lambda: eval(expr))

print("== ByteString deprecation ==")
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    print(isinstance(b"x", ByteString), isinstance(bytearray(), ByteString))
    print(isinstance("x", ByteString), issubclass(bytes, ByteString))
print([str(w.message) for w in caught], [w.category.__name__ for w in caught])
print(cabc.ByteString, cabc.ByteString.__mro__)
print(issubclass(bytes, cabc.ByteString), issubclass(memoryview, cabc.ByteString))

print("== subclassing an alias ==")


class Registry(Mapping[str, int]):
    def __init__(self):
        self._data = {"a": 1, "b": 2}

    def __getitem__(self, key):
        return self._data[key]

    def __iter__(self):
        return iter(self._data)

    def __len__(self):
        return len(self._data)


registry = Registry()
print(Registry.__mro__)
print(Registry.__orig_bases__, Registry.__parameters__)
print(sorted(registry.keys()), registry.get("b"), "a" in registry, len(registry))
print(isinstance(registry, Mapping), isinstance(registry, cabc.Mapping))


class Stack(Sequence[T]):
    def __init__(self, *items):
        self._items = list(items)

    def __getitem__(self, index):
        return self._items[index]

    def __len__(self):
        return len(self._items)


stack = Stack(1, 2, 3)
print(Stack.__mro__, Stack.__parameters__)
print(Stack[int], Stack[int](4, 5)._items, Stack[int](4).__orig_class__)
print(list(reversed(stack)), stack.index(2), stack.count(3))


class Counting(Iterator[int]):
    def __init__(self):
        self.n = 0

    def __next__(self):
        self.n += 1
        if self.n > 3:
            raise StopIteration
        return self.n


print(list(Counting()), Counting.__mro__)


class Bare(Iterable):
    def __iter__(self):
        return iter([1])


print(Bare.__mro__, Bare.__orig_bases__, Bare.__parameters__, list(Bare()))


class Both(Iterable[int], Generic[T]):
    def __iter__(self):
        return iter([])


print(Both.__mro__, Both.__parameters__)


class Two(Mapping[K, V], MutableSequence[T]):
    pass


print(Two.__mro__, Two.__parameters__)


class Managed(ContextManager[int]):
    def __exit__(self, *exc_info):
        return None


with Managed() as managed:
    print(type(managed).__name__, Managed.__mro__)


class NotGeneric(Hashable):
    def __hash__(self):
        return 1


print(NotGeneric.__mro__, NotGeneric.__parameters__)
print(Iterable.__mro_entries__((Iterable,)))
print(Iterable.__mro_entries__((Iterable, Generic)))
print(Iterable[int].__mro_entries__((Iterable[int],)))


def unlisted_typevar():
    class Bad(Iterable[T], Generic[K]):
        pass
    return Bad


show("unlisted type variable", unlisted_typevar)

print("== calling an alias ==")
for expr in [
    "Deque()", "Deque([1, 2])", "Deque[int]([1])", "Counter('aab')",
    "Counter[str]('aab').__orig_class__", "DefaultDict(int)",
    "DefaultDict[str, int](int)['x']", "OrderedDict(a=1)",
    "ChainMap({'a': 1})['a']",
]:
    show(expr, lambda: eval(expr))

print("== attribute surface ==")
for expr in [
    "Iterable.__origin__", "Iterable._nparams", "Iterable._name",
    "Iterable._inst", "Iterable.__foo__", "Iterable.nonexistent",
    "Iterable.__args__", "Iterable.__parameters__", "Iterable.__reduce__()",
    "copy.copy(Iterable) is Iterable", "copy.deepcopy(Iterable) is Iterable",
    "type(Iterable)", "type(Iterable[int])", "type(ByteString)",
    "typing._SpecialGenericAlias", "typing._DeprecatedGenericAlias",
    "issubclass(typing._DeprecatedGenericAlias, typing._SpecialGenericAlias)",
    "isinstance(Iterable[int], typing._GenericAlias)",
]:
    show(expr, lambda: eval(expr))

print("== Text / AnyStr ==")
print(Text, Text is str, repr(AnyStr), AnyStr.__name__, AnyStr.__module__)
print(AnyStr.__constraints__, AnyStr.__bound__, AnyStr.__covariant__, AnyStr.__contravariant__)
print(Iterable[AnyStr], Iterable[AnyStr].__parameters__, Iterable[AnyStr][bytes])

print("== ForwardRef ==")
for expr in [
    "ForwardRef('int')", "ForwardRef('x', module='m')",
    "ForwardRef('int') == ForwardRef('int')", "ForwardRef('int') == ForwardRef('str')",
    "ForwardRef('int') == 'int'", "hash(ForwardRef('a')) == hash(ForwardRef('a'))",
    "ForwardRef('int') | None", "ForwardRef('int').__forward_arg__",
    "ForwardRef('int').__forward_evaluated__", "ForwardRef('int').__forward_module__",
    "ForwardRef('int')._evaluate(globals(), None, recursive_guard=frozenset())",
    "ForwardRef(1)", "ForwardRef('1 +')", "get_args(Iterable['Stack'])",
]:
    show(expr, lambda: eval(expr))
