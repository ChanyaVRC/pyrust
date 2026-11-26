# Pins PEP 585 subscription of the `collections.abc` ABCs and `re.Pattern` /
# `re.Match` (issue #3326): CPython defines `__class_getitem__` on Awaitable,
# AsyncIterable, Iterable, Container, Callable, and MappingView, and every other
# generic ABC inherits it — so user subclasses (including subclasses of the
# `typing` aliases) subscript to a plain `types.GenericAlias`.
import collections.abc as cabc
import re
import types
from typing import Generic, Sequence, TypeVar, get_args, get_origin

T = TypeVar("T")


def show(label, thunk):
    try:
        print(label, "->", repr(thunk()))
    except Exception as exc:  # noqa: BLE001 - the error text is the point
        print(label, "-> raises", type(exc).__name__, exc)


NAMES = [
    "Awaitable", "Coroutine", "AsyncIterable", "AsyncIterator", "AsyncGenerator",
    "Hashable", "Iterable", "Iterator", "Generator", "Reversible", "Sized",
    "Container", "Collection", "Set", "MutableSet", "Mapping",
    "MutableMapping", "MappingView", "KeysView", "ItemsView", "ValuesView",
    "Sequence", "MutableSequence", "ByteString", "Buffer",
]
print("== own / inherited __class_getitem__ ==")
for name in NAMES:
    abc = getattr(cabc, name)
    print(name, "__class_getitem__" in vars(abc), hasattr(abc, "__class_getitem__"))

print("== subscription ==")
for expr in [
    "cabc.Iterable[int]", "cabc.Mapping[str, int]", "cabc.Sequence[str]",
    "cabc.MutableMapping[str, list[int]]", "cabc.Generator[int, str, bool]",
    "cabc.Awaitable[int]", "cabc.Coroutine[int, str, bool]",
    "cabc.AsyncIterator[int]", "cabc.KeysView[str]", "cabc.ItemsView[str, int]",
    "cabc.MappingView[int]", "cabc.Container[int]", "cabc.Set[int]",
    "cabc.ByteString[int]", "cabc.Iterable[T]", "cabc.Hashable[int]",
    "cabc.Sized[int]", "cabc.Buffer[int]", "re.Pattern[str]", "re.Match[bytes]",
    "re.Pattern[str] == re.Pattern[str]",
]:
    show(expr, lambda: eval(expr))

alias = cabc.Mapping[str, int]
print(type(alias) is types.GenericAlias, alias.__origin__, alias.__args__)
print(get_origin(alias), get_args(alias), cabc.Iterable[T].__parameters__)
print(type(re.Pattern[str]) is types.GenericAlias, get_origin(re.Match[str]))

print("== user subclasses ==")


class Bag(cabc.Iterable):
    def __iter__(self):
        return iter([1, 2])


class Stack(Sequence[T]):
    def __init__(self, *items):
        self._items = list(items)

    def __getitem__(self, index):
        return self._items[index]

    def __len__(self):
        return len(self._items)


class Grid(cabc.Mapping, Generic[T]):
    def __getitem__(self, key):
        raise KeyError(key)

    def __iter__(self):
        return iter(())

    def __len__(self):
        return 0


print(Bag[int], type(Bag[int]) is types.GenericAlias, list(Bag[int]()))
print(Stack[int], Stack[int, str], type(Stack[int]) is types.GenericAlias)
print(Stack[int](1, 2)._items, Stack[int](1).__orig_class__)
print(Grid[int], type(Grid[int]) is types.GenericAlias, Grid.__parameters__)
print(list[int]().__class__, dict[str, int](a=1))
