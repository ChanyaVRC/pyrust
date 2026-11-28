# collections.abc / _collections_abc — the 100% native-Rust module (#3480).
#
# Pins CPython 3.12.3's `Lib/_collections_abc.py` surface: `__all__`, every
# ABC's metaclass / MRO / `__abstractmethods__` / `__slots__`, the structural
# `__subclasshook__`s, the virtual registrations, every mixin method of
# concrete `Sequence` / `MutableSequence` / `Mapping` / `MutableMapping` /
# `Set` / `MutableSet` subclasses, the abstract-method bodies, the async
# mixins, `GenericAlias` / `_CallableGenericAlias` subscripting, `register()`,
# and interop with `typing`, `numbers`, `contextlib`, and `selectors`.
#
# Avoided on purpose (runtime-wide gaps, not module behaviour): reprs and
# `__doc__` of native methods, the source location of warnings raised from
# native code, and `repr(type(<generator>))`.

import asyncio
import collections
import collections.abc as cabc
import _collections_abc
import abc
import array
import types
import typing
import warnings
import weakref

from collections.abc import (
    AsyncGenerator, AsyncIterable, AsyncIterator, Awaitable, Buffer, ByteString,
    Callable, Collection, Container, Coroutine, Generator, Hashable, ItemsView,
    Iterable, Iterator, KeysView, Mapping, MappingView, MutableMapping,
    MutableSequence, MutableSet, Reversible, Sequence, Set, Sized, ValuesView,
)

warnings.simplefilter("ignore", DeprecationWarning)


def show(label, f):
    try:
        result = f()
    except Exception as e:
        result = f"!{type(e).__name__}: {e}"
    print(label, result)


print("== module surface ==")
print(cabc.__all__)
print(cabc.__all__ is _collections_abc.__all__, cabc.__name__, _collections_abc.__name__)
print(cabc.__doc__, repr(_collections_abc.__doc__))
print(sorted(k for k in vars(cabc) if not k.startswith("__")))
for name in ["ABCMeta", "abstractmethod", "GenericAlias", "EllipsisType", "FunctionType",
             "_check_methods", "_CallableGenericAlias", "_is_param_expr", "_type_repr",
             "_DeprecateByteStringMeta", "dict_keys", "dict_values", "dict_items",
             "mappingproxy", "generator", "coroutine", "async_generator"]:
    print(name, hasattr(_collections_abc, name), hasattr(cabc, name))
print(_collections_abc.ABCMeta is abc.ABCMeta, _collections_abc.GenericAlias is types.GenericAlias)
print(_collections_abc.dict_keys is type({}.keys()), _collections_abc.mappingproxy is types.MappingProxyType)
print(_collections_abc.list_iterator is type(iter([])), _collections_abc.zip_iterator is type(zip()))
for name in cabc.__all__:
    print(name, getattr(cabc, name) is getattr(_collections_abc, name))

print("== class shapes ==")
for name in cabc.__all__:
    cls = getattr(cabc, name)
    own = vars(cls)
    print(name, type(cls).__name__, cls.__module__, cls.__qualname__,
          [b.__name__ for b in cls.__mro__],
          sorted(cls.__abstractmethods__), cls.__slots__,
          "__class_getitem__" in own, "__subclasshook__" in own,
          isinstance(cls, abc.ABCMeta))
for name in cabc.__all__:
    cls = getattr(cabc, name)
    print(name, repr(cls.__doc__))
print(type(ByteString).__mro__ == (_collections_abc._DeprecateByteStringMeta, abc.ABCMeta, type, object))
print(Mapping.__reversed__, Mapping.__hash__, Set.__hash__, Sequence.__hash__ is object.__hash__)
print(MutableMapping._MutableMapping__marker.__class__ is object)
print(Set.__rand__ is Set.__and__, Set.__ror__ is Set.__or__, Set.__rxor__ is Set.__xor__)
for cls, name in [(Sequence, "__getitem__"), (Sequence, "index"), (Hashable, "__hash__"),
                  (Iterator, "__next__"), (Iterator, "__iter__"), (MutableSequence, "insert"),
                  (Generator, "send"), (Generator, "close"), (Buffer, "__buffer__")]:
    meth = getattr(cls, name)
    print(cls.__name__, name, getattr(meth, "__isabstractmethod__", False), meth.__name__)

print("== isinstance / issubclass matrix ==")


def gen():
    yield 1


async def coro():
    pass


async def agen():
    yield 1


class Plain:
    def method(self):
        pass


c = coro()
VALUES = {
    "int": 1, "float": 1.0, "complex": 1j, "str": "s", "bytes": b"b",
    "bytearray": bytearray(b"x"), "memoryview": memoryview(b"m"), "list": [1],
    "tuple": (1,), "dict": {1: 2}, "set": {1}, "frozenset": frozenset({1}),
    "range": range(3), "None": None, "Ellipsis": ..., "bool": True, "type": int,
    "object": object(), "keys": {}.keys(), "values": {}.values(), "items": {}.items(),
    "mappingproxy": types.MappingProxyType({}), "gen": gen(), "coro": c, "agen": agen(),
    "list_iter": iter([]), "tuple_iter": iter(()), "str_iter": iter(""),
    "bytes_iter": iter(b""), "range_iter": iter(range(3)), "set_iter": iter(set()),
    "dict_keyiter": iter({}), "dict_itemiter": iter({}.items()), "reversed": reversed([]),
    "zip": zip(), "map": map(len, []), "enumerate": enumerate([]),
    "callable_iter": iter(lambda: 1, 1), "func": lambda: 0, "method": Plain().method,
    "builtin": len, "builtin_method": [].append, "method_descr": list.append,
    "module": types, "deque": collections.deque(), "OrderedDict": collections.OrderedDict(),
    "Counter": collections.Counter(), "ChainMap": collections.ChainMap(),
    "UserDict": collections.UserDict(), "UserList": collections.UserList(),
    "UserString": collections.UserString(""), "array": array.array("i"),
    "WeakSet": weakref.WeakSet(), "WeakKeyDict": weakref.WeakKeyDictionary(),
    "SimpleNamespace": types.SimpleNamespace(), "odict_keys": collections.OrderedDict().keys(),
    "odict_items": collections.OrderedDict().items(), "Plain": Plain(), "exc": ValueError(),
}
ABCS = ["Hashable", "Awaitable", "Coroutine", "AsyncIterable", "AsyncIterator",
        "AsyncGenerator", "Iterable", "Iterator", "Reversible", "Generator", "Sized",
        "Container", "Collection", "Callable", "Buffer", "Set", "MutableSet", "Mapping",
        "MutableMapping", "MappingView", "KeysView", "ItemsView", "ValuesView", "Sequence",
        "MutableSequence", "ByteString"]
print("".join(a[:2] for a in ABCS))
for label, value in VALUES.items():
    row = []
    for name in ABCS:
        cls = getattr(cabc, name)
        row.append(("1" if isinstance(value, cls) else "0")
                   + ("1" if issubclass(type(value), cls) else "0"))
    print(f"{label:16}", " ".join(row))
c.close()

print("== structural hooks ==")


class OnlyLen:
    def __len__(self):
        return 0


class OnlyIter:
    def __iter__(self):
        return iter(())


class IterNone(OnlyIter):
    __iter__ = None


class Unhashable:
    __hash__ = None


class EqNoHash:
    def __eq__(self, other):
        return True


class ListSub(list):
    pass


class DuckMapping:
    def __getitem__(self, key):
        raise KeyError(key)

    def __iter__(self):
        return iter(())

    def __len__(self):
        return 0


for cls in [OnlyLen, OnlyIter, IterNone, Unhashable, EqNoHash, ListSub, DuckMapping]:
    print(cls.__name__, [issubclass(cls, abc_cls) for abc_cls in
                         (Sized, Iterable, Hashable, Collection, Mapping, Sequence, Reversible)])
print(Iterable.__subclasshook__(OnlyIter), Iterable.__subclasshook__(OnlyLen))
print(Sequence.__subclasshook__(list), Collection.__subclasshook__(list))
print(Sized.__subclasshook__(OnlyLen), Hashable.__subclasshook__(Unhashable))
print(_collections_abc._check_methods(OnlyLen, "__len__"), _collections_abc._check_methods(IterNone, "__iter__"))
print(_collections_abc._check_methods(list, "__len__", "__iter__", "__missing__"))


class MyIterable(Iterable):
    def __iter__(self):
        return iter([1])


print(MyIterable.__subclasshook__(OnlyIter), issubclass(OnlyIter, MyIterable))

print("== abstract instantiation ==")


def subclass_of(base):
    class Sub(base):
        pass

    return Sub


for abc_cls in [Hashable, Awaitable, Coroutine, AsyncIterable, AsyncIterator, AsyncGenerator,
                Iterable, Iterator, Reversible, Generator, Sized, Container, Collection,
                Buffer, Callable, Set, MutableSet, Mapping, MutableMapping, Sequence,
                MutableSequence, ByteString]:
    Sub = subclass_of(abc_cls)
    show(abc_cls.__name__, lambda: Sub())
show("MappingView", lambda: type(MappingView({})).__name__)


class PartialSeq(Sequence):
    def __len__(self):
        return 0


show("partial", lambda: PartialSeq())
print(sorted(PartialSeq.__abstractmethods__))

print("== Sequence ==")


class Seq(Sequence):
    def __init__(self, data):
        self.data = list(data)

    def __getitem__(self, i):
        return self.data[i]

    def __len__(self):
        return len(self.data)


s = Seq([1, 2, 3, 2, 1])
print(list(s), list(reversed(s)), 2 in s, 9 in s)
print(s.index(2), s.index(2, 2), s.index(1, -1), s.index(1, -100), s.count(2), s.count(9))
show("index missing", lambda: s.index(9))
show("index stop", lambda: s.index(1, 1, 3))
show("index neg stop", lambda: s.index(3, 0, -1))
show("index noargs", lambda: s.index())
show("count noargs", lambda: s.count())
show("index kwargs", lambda: s.index(value=3, start=1, stop=None))
it = iter(s)
print(type(it).__name__, next(it), next(it), list(it))
rit = reversed(s)
print(type(rit).__name__, list(rit))
print(isinstance(s, Sequence), isinstance(s, Reversible), isinstance(s, MutableSequence))
print(Seq.__abstractmethods__, [c.__name__ for c in Seq.__mro__])


NAN = float("nan")


class NaNSeq(Sequence):
    def __getitem__(self, i):
        return [NAN][i]

    def __len__(self):
        return 1


# `v is value or v == value`: identity wins even though NaN != NaN.
print(NAN in NaNSeq(), NaNSeq().count(NAN), NaNSeq().index(NAN), float("nan") in NaNSeq())

print("== MutableSequence ==")


class MSeq(MutableSequence):
    def __init__(self, data=()):
        self.data = list(data)

    def __getitem__(self, i):
        return self.data[i]

    def __setitem__(self, i, v):
        self.data[i] = v

    def __delitem__(self, i):
        del self.data[i]

    def __len__(self):
        return len(self.data)

    def insert(self, i, v):
        self.data.insert(i, v)

    def __repr__(self):
        return f"MSeq({self.data})"


m = MSeq([1, 2, 3])
m.append(4)
print(m)
m.extend([5, 6])
print(m)
m.extend(m)
print(m)
m.reverse()
print(m)
print(m.pop(), m.pop(0), m)
m.remove(3)
print(m)
show("remove missing", lambda: m.remove(99))
m += (7, 8)
print(m, type(m).__name__)
m.clear()
print(m)
show("pop empty", lambda: m.pop())
show("append noargs", lambda: m.append())
show("extend int", lambda: m.extend(5))

print("== Mapping / MutableMapping ==")


class D(MutableMapping):
    def __init__(self, *args, **kwargs):
        self.d = {}
        self.update(*args, **kwargs)

    def __getitem__(self, key):
        return self.d[key]

    def __setitem__(self, key, value):
        self.d[key] = value

    def __delitem__(self, key):
        del self.d[key]

    def __iter__(self):
        return iter(self.d)

    def __len__(self):
        return len(self.d)

    def __repr__(self):
        return f"D({self.d})"


d = D({"a": 1}, b=2)
print(d, d.get("a"), d.get("z"), d.get("z", 0), "a" in d, "z" in d)
print(d.keys(), d.items(), d.values())
print(list(d.keys()), list(d.items()), list(d.values()))
print(len(d.keys()), "a" in d.keys(), ("a", 1) in d.items(), ("a", 2) in d.items())
print(2 in d.values(), 5 in d.values(), len(d.values()), len(d.items()))
print(sorted(d.keys() & {"a", "q"}), sorted(d.keys() | {"q"}), sorted(d.keys() - {"a"}))
print(sorted(d.keys() ^ {"a", "q"}), sorted(d.items() - {("a", 1)}), type(d.keys() & {"a"}).__name__)
print(d.keys() <= {"a", "b", "c"}, d.keys() == {"a", "b"}, d.keys() < {"a", "b"}, d.keys().isdisjoint({"z"}))
print(d == {"a": 1, "b": 2}, d == D(a=1, b=2), d == {"a": 1}, d != {"a": 1}, d == [1])
print(d.setdefault("c", 3), d.setdefault("c", 4), d.setdefault("n"), d)
print(d.pop("c"), d.pop("zz", "dflt"), d.pop("n"), d)
show("pop missing", lambda: d.pop("zz"))
show("pop explicit marker", lambda: d.pop("zz", MutableMapping._MutableMapping__marker))
print(d.popitem(), d)
d.update([("x", 1)], y=2)
print(d)
d.update({"z": 0})
print(d)


class KeysOnly:
    def keys(self):
        return ["k1", "k2"]

    def __getitem__(self, key):
        return key.upper()


d.update(KeysOnly())
print(d)
d.update(self=1, other=2)
print(d)
d.update(D(m=1))
print(d)
show("update too many", lambda: d.update({}, {}))
show("update triple", lambda: d.update([(1, 2, 3)]))
show("update single", lambda: d.update([(1,)]))
show("update scalar", lambda: d.update([1]))
show("update noargs", lambda: MutableMapping.update())
d.clear()
print(d)
show("popitem empty", lambda: d.popitem())
try:
    d.popitem()
except KeyError as e:
    print("from None", e.__suppress_context__, e.__cause__, e.args)
show("reversed", lambda: reversed(d))
show("hash", lambda: hash(d))
show("get noargs", lambda: d.get())
print(isinstance(d.keys(), KeysView), isinstance(d.keys(), Set), isinstance(d.values(), ValuesView))
print(isinstance(d.items(), ItemsView), isinstance(d.values(), Set), isinstance(d, Mapping))
print(repr(KeysView({1: 2})), repr(ValuesView([1])), repr(ItemsView({})))
print(KeysView({1: 2})._mapping, len(MappingView([1, 2, 3])), MappingView.__slots__)
show("items contains scalar", lambda: 1 in D(a=1).items())
show("items contains triple", lambda: (1, 2, 3) in D(a=1).items())
show("view no args", lambda: KeysView())
show("keysview from_iterable", lambda: KeysView._from_iterable([1, 1, 2]))


class RO(Mapping):
    def __init__(self, data):
        self.data = data

    def __getitem__(self, key):
        return self.data[key]

    def __iter__(self):
        return iter(self.data)

    def __len__(self):
        return len(self.data)


ro = RO({"x": 1, "y": 2})
print(dict(ro), ro.get("x"), ro.get("q", -1), "y" in ro, list(ro.items()), ro == {"x": 1, "y": 2})
print(isinstance(ro, MutableMapping), hasattr(ro, "pop"), {**ro})
match ro:
    case {"x": x}:
        print("match mapping", x)
    case _:
        print("no match")

print("== Set ==")


class FS(Set):
    def __init__(self, it=()):
        self.data = []
        for x in it:
            if x not in self.data:
                self.data.append(x)

    def __contains__(self, x):
        return x in self.data

    def __iter__(self):
        return iter(self.data)

    def __len__(self):
        return len(self.data)

    def __repr__(self):
        return f"FS({sorted(self.data)})"

    __hash__ = Set._hash


a, b = FS([1, 2, 3]), FS([2, 3, 4])
print(a & b, a | b, a - b, b - a, a ^ b, a & [3, 9], [3, 9] & a, [9] | a)
print(a <= b, a < FS([1, 2, 3, 4]), a >= FS([1]), a > FS([1]), a == FS([3, 2, 1]), a != b)
print(a < a, a > a, a <= a, a >= a)
print(a.isdisjoint([9]), a.isdisjoint([1]))
print(a <= {1, 2, 3}, {1, 2, 3} >= a, a == {1, 2, 3}, a == frozenset({1, 2, 3}), {1, 2} < a)
show("and int", lambda: a & 5)
show("sub list", lambda: a - [1])
show("rsub list", lambda: [1, 2, 9] - a)
show("xor list", lambda: a ^ [1, 7])
show("le list", lambda: a <= [1, 2, 3])
show("eq list", lambda: a == [1, 2, 3])
print(hash(FS([1, 2, 3])) == hash(frozenset({1, 2, 3})), hash(FS()) == hash(frozenset()))
for items in ([1, -1, 2**62, -2**63, 0], ["x", "y"], [(1, 2), None, 3.5], list(range(50))):
    print(FS(items)._hash() == hash(frozenset(items)))
show("from_iterable", lambda: FS._from_iterable([3, 3, 1]))

print("== MutableSet ==")


class MS(MutableSet):
    def __init__(self, it=()):
        self.data = set(it)

    def __contains__(self, x):
        return x in self.data

    def __iter__(self):
        return iter(sorted(self.data))

    def __len__(self):
        return len(self.data)

    def add(self, x):
        self.data.add(x)

    def discard(self, x):
        self.data.discard(x)

    def __repr__(self):
        return f"MS({sorted(self.data)})"


ms = MS([1, 2, 3])
ms.remove(1)
print(ms)
show("remove missing", lambda: ms.remove(1))
print(ms.pop(), ms)
ms |= [5, 6]
print(ms)
ms &= [5, 6, 7]
print(ms)
ms ^= [6, 8]
print(ms)
ms ^= MS([5, 9])
print(ms)
ms -= [8]
print(ms)
ms ^= ms
print(ms)
ms |= [1]
ms -= ms
print(ms)
ms.add(3)
ms.clear()
print(ms)
show("pop empty", lambda: ms.pop())
try:
    ms.pop()
except KeyError as e:
    print("from None", e.__suppress_context__, e.args)

print("== Iterator / Generator / Coroutine ==")


class It(Iterator):
    def __init__(self):
        self.n = 0

    def __next__(self):
        self.n += 1
        if self.n > 3:
            raise StopIteration
        return self.n


i = It()
print(iter(i) is i, list(It()))


class Gen(Generator):
    def __init__(self):
        self.n = 0
        self.thrown = []

    def send(self, value):
        self.n += 1
        if self.n > 2:
            raise StopIteration
        return self.n

    def throw(self, typ, val=None, tb=None):
        self.thrown.append(typ.__name__)
        raise typ


g = Gen()
print(next(g), list(g), g.close(), g.thrown)


class IgnoringGen(Gen):
    def throw(self, *args):
        return 1


show("ignored close", lambda: IgnoringGen().close())


class Coro(Coroutine):
    def __await__(self):
        return iter(())

    def send(self, value):
        raise StopIteration(value)

    def throw(self, typ, val=None, tb=None):
        raise StopIteration


class IgnoringCoro(Coro):
    def throw(self, *args):
        return 1


print(Coro().close(), isinstance(Coro(), Awaitable))
show("ignored coro close", lambda: IgnoringCoro().close())
show("Generator.throw class", lambda: Generator.throw(g, ValueError))
show("Generator.throw val", lambda: Generator.throw(g, ValueError, ValueError("x")))
show("Generator.throw inst", lambda: Generator.throw(g, KeyError("k")))
show("Generator.throw tb", lambda: Generator.throw(g, ValueError, None, None))
show("Coroutine.send", lambda: Coroutine.send(None, 1))
show("Iterator.__next__", lambda: Iterator.__next__(None))
show("Sized.__len__", lambda: Sized.__len__(None))
show("Hashable.__hash__", lambda: Hashable.__hash__(None))
show("Container.__contains__", lambda: Container.__contains__(None, 1))
show("Callable.__call__", lambda: Callable.__call__(None, 1, x=2))
show("Iterable.__iter__", lambda: list(Iterable.__iter__(None)))
show("Reversible.__reversed__", lambda: list(Reversible.__reversed__(None)))
show("Awaitable.__await__", lambda: list(Awaitable.__await__(None)))
show("AsyncIterable.__aiter__", lambda: AsyncIterable.__aiter__(None))
show("Mapping.__getitem__", lambda: Mapping.__getitem__(None, 1))
show("MutableMapping.__setitem__", lambda: MutableMapping.__setitem__(None, 1, 2))
show("Sequence.__getitem__", lambda: Sequence.__getitem__(None, 1))
show("MutableSequence.insert", lambda: MutableSequence.insert(None, 0, 1))
show("Buffer.__buffer__", lambda: Buffer.__buffer__(None, 0))
show("Buffer.__buffer__ kw", lambda: Buffer.__buffer__(None, flags=0))
show("MutableSet.add", lambda: MutableSet.add(None, 0))
show("Sized.__len__ noargs", lambda: Sized.__len__())


class SuperLen(Sized):
    def __len__(self):
        return super().__len__() + 5


print(len(SuperLen()), bool(SuperLen()))

print("== async ==")


class AG(AsyncGenerator):
    def __init__(self):
        self.n = 0

    async def asend(self, value):
        self.n += 1
        if self.n > 2:
            raise StopAsyncIteration
        return self.n

    async def athrow(self, typ, val=None, tb=None):
        raise typ


class IgnoringAG(AG):
    async def athrow(self, *args):
        return 1


class AIt(AsyncIterator):
    def __init__(self):
        self.left = 2

    async def __anext__(self):
        if not self.left:
            raise StopAsyncIteration
        self.left -= 1
        return self.left


async def main():
    ag = AG()
    print(ag.__aiter__() is ag, await ag.__anext__(), await ag.__anext__())
    try:
        await ag.__anext__()
    except StopAsyncIteration:
        print("stop")
    print(await ag.aclose())
    print([x async for x in AG()], [x async for x in AIt()])
    try:
        await IgnoringAG().aclose()
    except RuntimeError as e:
        print("RuntimeError", e)
    for name, awaitable in [("anext", AsyncIterator.__anext__(None)),
                            ("asend", AsyncGenerator.asend(None, 1)),
                            ("athrow", AsyncGenerator.athrow(None, KeyError))]:
        try:
            await awaitable
        except (StopAsyncIteration, KeyError) as e:
            print(name, type(e).__name__)
    print(isinstance(AG(), AsyncIterable), isinstance(AIt(), AsyncGenerator))


asyncio.run(main())

print("== generic aliases ==")
for alias in [Sequence[int], Mapping[str, int], Iterable[int], MutableSequence[str],
              Set[int], KeysView[str], ItemsView[str, int], ValuesView[int],
              AsyncIterator[int], Generator[int, None, None], Coroutine[int, str, bool],
              Container[int], Awaitable[int], AsyncGenerator[int, None], Reversible[int],
              MappingView[int], Callable[[int, str], bool], Callable[..., int],
              Callable[[], None], Callable[[list[int]], dict[str, int]]]:
    print(alias, type(alias).__name__, alias.__origin__.__name__, alias.__args__,
          isinstance(alias, types.GenericAlias))
for expr in ["Hashable[int]", "Sized[int]", "Buffer[int]", "Callable[int]",
             "Callable[int, str]", "Callable[[int], str, bytes]"]:
    show(expr, lambda: eval(expr))
print(Callable[[int], str] == Callable[[int], str], Callable[[int], str] == Callable[[str], str])
print(hash(Callable[[int], str]) == hash(Callable[[int], str]), Sequence[int] == Sequence[int])
print(Callable.__class_getitem__(((int,), str)), Iterable.__class_getitem__(int))
print(cabc._CallableGenericAlias(Callable, ([int], str)).__args__)
print(issubclass(cabc._CallableGenericAlias, types.GenericAlias), cabc._CallableGenericAlias.__module__)
print(typing.get_args(Callable[[int, str], bool]), typing.get_args(Callable[..., int]),
      typing.get_args(Callable[[], int]))
print(typing.get_origin(Callable[[int], str]) is Callable, typing.get_args(Mapping[str, int]),
      typing.get_origin(Sequence[int]) is Sequence)
print(_collections_abc._is_param_expr(...), _collections_abc._is_param_expr([int]),
      _collections_abc._is_param_expr(int), _collections_abc._is_param_expr(typing.ParamSpec("P")))
print(_collections_abc._type_repr(int), _collections_abc._type_repr(Mapping),
      _collections_abc._type_repr(...), _collections_abc._type_repr(gen),
      _collections_abc._type_repr("x"))


class Pair(Mapping[str, int]):
    def __getitem__(self, key):
        return 1

    def __iter__(self):
        return iter(["a"])

    def __len__(self):
        return 1


print(Pair.__orig_bases__ if hasattr(Pair, "__orig_bases__") else "no orig", dict(Pair()))
print([c.__name__ for c in Pair.__mro__])

print("== register ==")


class Duck:
    def __len__(self):
        return 0


print(isinstance(Duck(), Sized), isinstance(Duck(), Sequence))
print(Sequence.register(Duck) is Duck, isinstance(Duck(), Sequence), issubclass(Duck, Reversible))
print(issubclass(Duck, MutableSequence), issubclass(Duck, Collection), Duck.__mro__)
print(Hashable.register(Unhashable) is Unhashable, isinstance(Unhashable(), Hashable))
show("register non-class", lambda: Sequence.register(5))
show("register cycle", lambda: Sized.register(Collection))


@MutableMapping.register
class Decorated:
    pass


print(Decorated.__name__, issubclass(Decorated, Mapping), issubclass(Decorated, Sized))


class MyABC(abc.ABC):
    pass


MyABC.register(type(gen()))
print(isinstance(gen(), MyABC), issubclass(type(gen()), MyABC), isinstance(iter([]), MyABC))

print("== interop ==")
print(isinstance([], typing.Sequence), isinstance({}, typing.Mapping), issubclass(list, typing.MutableSequence))
print(typing.Sequence[int], typing.Mapping[str, int].__origin__ is Mapping, typing.Callable[[int], str])
print(typing.Iterable.__origin__ is Iterable, typing.AbstractSet.__origin__ is Set)


class TS(typing.Sequence[int]):
    def __getitem__(self, i):
        return [5][i]

    def __len__(self):
        return 1


print(list(TS()), isinstance(TS(), Sequence), [c.__name__ for c in TS.__mro__])
import numbers
print(isinstance(1, numbers.Integral), isinstance(1, Hashable), type(numbers.Number).__name__)
import contextlib
print(issubclass(contextlib.AbstractContextManager, Iterable), isinstance(contextlib.nullcontext(), Hashable))
import selectors
print(issubclass(selectors._SelectorMapping, Mapping), issubclass(collections.OrderedDict, MutableMapping))
print(issubclass(collections.UserList, MutableSequence), issubclass(collections.UserString, Sequence))
print(issubclass(collections.deque, MutableSequence), issubclass(array.array, MutableSequence))
print(issubclass(weakref.WeakSet, MutableSet), issubclass(collections.Counter, Mapping))

print("== handled exceptions leave no traceback frames ==")
import traceback


def frame_names(exc):
    return [frame.name for frame in traceback.extract_tb(exc.__traceback__)]


ro.get("missing")
print("missing" in ro, list(Seq([1, 2])), list(reversed(Seq([1]))), D(a=1).pop("x", None))
D(a=1).setdefault("q", 1)
D(a=1).clear()
MSeq([1, 2]).clear()
MS([1, 2]).clear()
Gen().close()
print(Seq([1, 2, 3]).index(3), ("k", 1) in D(a=1).items(), list(D(a=1).items()))
try:
    raise RuntimeError("fresh")
except RuntimeError as e:
    print(frame_names(e))


def raiser():
    raise LookupError("inner")


try:
    raiser()
except LookupError as e:
    print(frame_names(e))

print("== ByteString ==")
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    print(isinstance(b"", ByteString), isinstance(bytearray(), ByteString), isinstance("", ByteString))
    print(issubclass(bytes, ByteString), issubclass(str, ByteString), issubclass(memoryview, ByteString))

    class BS(ByteString):
        pass

    print(type(BS).__name__, BS.__abstractmethods__ == frozenset({"__getitem__", "__len__"}))
for w in caught:
    print(w.category.__name__, w.message)
