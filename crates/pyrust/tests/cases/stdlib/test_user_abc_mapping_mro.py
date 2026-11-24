# Issue #3020: User* / ChainMap ABC bases, Mapping.__eq__, and MRO object-last.
#
# Verifies:
#   1. dict == UserDict / ChainMap (both directions, == and !=) via Mapping.__eq__.
#   2. UserDict, ChainMap, UserList, UserString inherit their CPython collections.abc
#      bases and have exact CPython __mro__ tuples.
#   3. Registered ABCs are NOT spliced into __bases__ or __mro__ (OrderedDict,
#      Counter, defaultdict, dict, list, … keep clean MROs ending in `object`)
#      while isinstance / issubclass against collections.abc remain True.
#   4. `object` is always last in __mro__ across a broad set of builtin, stdlib,
#      ABC, and user-defined classes.
#   5. Mapping.__eq__ lives in vars(collections.abc.Mapping) and returns
#      NotImplemented for non-Mapping operands; sibling ABC mixins work on custom
#      subclasses.
#   6. Regression guards: dict.__eq__(d, non_dict_mapping) is NotImplemented,
#      dict == OrderedDict, dict == Counter, list == UserList, str == UserString.

import abc as stdlib_abc
import collections
import collections.abc as abc
import decimal
import enum
import fractions
import functools
import io
import pathlib
import types
from collections import (
    ChainMap,
    Counter,
    OrderedDict,
    UserDict,
    UserList,
    UserString,
    defaultdict,
    deque,
)

# ── 1. Equality against non-dict mappings ────────────────────────────────────
d = {"a": 1}
u = UserDict(a=1)
u_diff = UserDict(a=2)
cm = ChainMap({"a": 1})
cm_multi = ChainMap({"a": 1}, {"a": 99, "b": 2})
cm_diff = ChainMap({"a": 2})

print("dict == UserDict:", d == u, u == d, d != u, u != d)
print("dict == UserDict diff:", d == u_diff, u_diff == d, d != u_diff, u_diff != d)
print("dict == ChainMap:", d == cm, cm == d, d != cm, cm != d)
print(
    "dict == ChainMap multi:",
    {"a": 1, "b": 2} == cm_multi,
    cm_multi == {"a": 1, "b": 2},
    cm_multi == ChainMap({"a": 1, "b": 2}),
    cm_multi == UserDict(a=1, b=2),
    UserDict(a=1, b=2) == cm_multi,
)
print("dict == ChainMap diff:", d == cm_diff, cm_diff == d, d != cm_diff, cm_diff != d)
print("UserDict == non-mapping:", u == [("a", 1)], u != [("a", 1)], u == 42, u != 42)
print("ChainMap == non-mapping:", cm == [("a", 1)], cm != [("a", 1)], cm == 42, cm != 42)

# Cross-type mapping equality (OrderedDict, Counter, defaultdict, MappingProxyType)
od = OrderedDict([("a", 1)])
ct = Counter({"a": 1})
dd = defaultdict(int, a=1)
mp = types.MappingProxyType({"a": 1})
print("OrderedDict == UserDict:", od == u, u == od, od != u, u != od)
print("Counter == UserDict:", ct == u, u == ct)
print("defaultdict == UserDict:", dd == u, u == dd)
print("MappingProxyType == UserDict:", mp == u, u == mp)
print("MappingProxyType == ChainMap:", mp == cm, cm == mp)

# ── 2. dict.__eq__ regression guard ──────────────────────────────────────────
class CustomMapping(abc.Mapping):
    def __init__(self, items):
        self._items = dict(items)

    def __getitem__(self, key):
        return self._items[key]

    def __iter__(self):
        return iter(self._items)

    def __len__(self):
        return len(self._items)


cust = CustomMapping({"a": 1})
print(
    "dict.__eq__ returns NotImplemented:",
    dict.__eq__(d, u) is NotImplemented,
    dict.__eq__(d, cm) is NotImplemented,
    dict.__eq__(d, cust) is NotImplemented,
)
print("dict == CustomMapping:", d == cust, cust == d, d != cust, cust != d)

# ── 3. Regression guards for already-working comparisons ─────────────────────
print("dict == OrderedDict:", d == od, od == d)
print("dict == Counter:", d == ct, ct == d)
print("list == UserList:", [1, 2] == UserList([1, 2]), UserList([1, 2]) == [1, 2])
print("str == UserString:", "abc" == UserString("abc"), UserString("abc") == "abc")

# ── 4. Mapping.__eq__ surface & direct invocation ────────────────────────────
print(
    "Mapping.__eq__ surface:",
    "__eq__" in vars(abc.Mapping),
    UserDict.__eq__ is abc.Mapping.__eq__,
    ChainMap.__eq__ is abc.Mapping.__eq__,
    abc.Mapping.__eq__(u, d),
    abc.Mapping.__eq__(u, {"a": 2}),
    abc.Mapping.__eq__(u, [("a", 1)]) is NotImplemented,
    abc.Mapping.__eq__(cm, d),
)

# ── 5. isinstance / issubclass checks ────────────────────────────────────────
print(
    "UserDict ABCs:",
    isinstance(u, abc.MutableMapping),
    isinstance(u, abc.Mapping),
    isinstance(u, abc.Collection),
    isinstance(u, abc.Sized),
    isinstance(u, abc.Iterable),
    isinstance(u, abc.Container),
    isinstance(u, abc.Reversible),
    isinstance(u, abc.Hashable),
)
print(
    "ChainMap ABCs:",
    isinstance(cm, abc.MutableMapping),
    isinstance(cm, abc.Mapping),
    isinstance(cm, abc.Collection),
    isinstance(cm, abc.Hashable),
)
print(
    "UserList ABCs:",
    isinstance(UserList([1]), abc.MutableSequence),
    isinstance(UserList([1]), abc.Sequence),
    isinstance(UserList([1]), abc.Reversible),
    isinstance(UserList([1]), abc.Collection),
    isinstance(UserList([1]), abc.Hashable),
)
print(
    "UserString ABCs:",
    isinstance(UserString("a"), abc.Sequence),
    isinstance(UserString("a"), abc.MutableSequence),
    isinstance(UserString("a"), abc.Reversible),
    isinstance(UserString("a"), abc.Collection),
    isinstance(UserString("a"), abc.Hashable),
)
print(
    "Registered dict subclasses:",
    isinstance(od, abc.Mapping),
    isinstance(od, abc.MutableMapping),
    isinstance(od, abc.Collection),
    isinstance(od, abc.Reversible),
    isinstance(od, abc.Hashable),
    issubclass(OrderedDict, abc.Mapping),
    isinstance(ct, abc.Mapping),
    isinstance(ct, abc.MutableMapping),
    isinstance(dd, abc.Mapping),
    isinstance(dd, abc.MutableMapping),
    isinstance(mp, abc.Mapping),
    isinstance(mp, abc.MutableMapping),
    isinstance(deque(), abc.MutableSequence),
    isinstance(deque(), abc.Sequence),
)

# ── 6. Exact __mro__ tuples ──────────────────────────────────────────────────
expected_mapping_mro = (
    abc.MutableMapping,
    abc.Mapping,
    abc.Collection,
    abc.Sized,
    abc.Iterable,
    abc.Container,
    object,
)
expected_mut_seq_mro = (
    abc.MutableSequence,
    abc.Sequence,
    abc.Reversible,
    abc.Collection,
    abc.Sized,
    abc.Iterable,
    abc.Container,
    object,
)
expected_seq_mro = (
    abc.Sequence,
    abc.Reversible,
    abc.Collection,
    abc.Sized,
    abc.Iterable,
    abc.Container,
    object,
)

print("UserDict MRO exact:", UserDict.__mro__ == (UserDict,) + expected_mapping_mro)
print("ChainMap MRO exact:", ChainMap.__mro__ == (ChainMap,) + expected_mapping_mro)
print("UserList MRO exact:", UserList.__mro__ == (UserList,) + expected_mut_seq_mro)
print("UserString MRO exact:", UserString.__mro__ == (UserString,) + expected_seq_mro)

print("OrderedDict MRO exact:", OrderedDict.__mro__ == (OrderedDict, dict, object))
print("Counter MRO exact:", Counter.__mro__ == (Counter, dict, object))
print("defaultdict MRO exact:", defaultdict.__mro__ == (defaultdict, dict, object))
print("deque MRO exact:", deque.__mro__ == (deque, object))
print("dict MRO & bases:", dict.__mro__ == (dict, object), dict.__bases__ == (object,))
print("list MRO & bases:", list.__mro__ == (list, object), list.__bases__ == (object,))
print("tuple MRO & bases:", tuple.__mro__ == (tuple, object), tuple.__bases__ == (object,))
print("str MRO & bases:", str.__mro__ == (str, object), str.__bases__ == (object,))
print("set MRO & bases:", set.__mro__ == (set, object), set.__bases__ == (object,))
print(
    "frozenset MRO & bases:",
    frozenset.__mro__ == (frozenset, object),
    frozenset.__bases__ == (object,),
)
print("range MRO & bases:", range.__mro__ == (range, object), range.__bases__ == (object,))

# ── 7. `object` is ALWAYS last in __mro__ across builtin & stdlib classes ────
class MixA:
    pass


class MixB(MixA):
    pass


class MixC(MixA):
    pass


class Diamond(MixB, MixC):
    pass


class IntMix(int, MixA):
    pass


class DictMix(dict, MixA):
    pass


broad_classes = [
    object,
    type,
    int,
    bool,
    float,
    complex,
    str,
    bytes,
    bytearray,
    list,
    tuple,
    dict,
    set,
    frozenset,
    range,
    slice,
    BaseException,
    Exception,
    ValueError,
    KeyError,
    TypeError,
    OrderedDict,
    Counter,
    defaultdict,
    deque,
    UserDict,
    ChainMap,
    UserList,
    UserString,
    abc.Container,
    abc.Hashable,
    abc.Iterable,
    abc.Iterator,
    abc.Reversible,
    abc.Generator,
    abc.Sized,
    abc.Collection,
    abc.Callable,
    abc.Sequence,
    abc.MutableSequence,
    abc.Set,
    abc.MutableSet,
    abc.Mapping,
    abc.MutableMapping,
    abc.MappingView,
    abc.KeysView,
    abc.ItemsView,
    abc.ValuesView,
    abc.Awaitable,
    abc.Coroutine,
    abc.AsyncIterable,
    abc.AsyncIterator,
    abc.AsyncGenerator,
    abc.Buffer,
    types.MappingProxyType,
    types.SimpleNamespace,
    stdlib_abc.ABC,
    stdlib_abc.ABCMeta,
    enum.Enum,
    enum.IntEnum,
    io.StringIO,
    io.BytesIO,
    fractions.Fraction,
    decimal.Decimal,
    pathlib.Path,
    functools.partial,
    Diamond,
    IntMix,
    DictMix,
    CustomMapping,
]

bad_mro = [cls.__name__ for cls in broad_classes if cls.__mro__[-1] is not object]
print("all object-last:", len(bad_mro) == 0, bad_mro)

# Inconsistent base order `(object, MixA)` must raise TypeError
try:
    class BadOrder(object, MixA):
        pass
    print("BadOrder unexpectedly succeeded")
except TypeError:
    print("BadOrder raised TypeError")

# ── 8. Sibling ABC concrete mixins work on custom subclasses ─────────────────
print(
    "CustomMapping mixins:",
    cust.get("a"),
    cust.get("z", 9),
    "a" in cust,
    "z" in cust,
    list(cust.keys()),
    list(cust.items()),
    list(cust.values()),
    isinstance(cust.keys(), abc.KeysView),
    isinstance(cust.items(), abc.ItemsView),
    isinstance(cust.values(), abc.ValuesView),
)


class CustomMutableMapping(abc.MutableMapping):
    def __init__(self, initial=()):
        self._d = dict(initial)

    def __getitem__(self, k):
        return self._d[k]

    def __setitem__(self, k, v):
        self._d[k] = v

    def __delitem__(self, k):
        del self._d[k]

    def __iter__(self):
        return iter(self._d)

    def __len__(self):
        return len(self._d)


cmm = CustomMutableMapping({"a": 1})
cmm.update({"b": 2}, c=3)
print("CustomMutableMapping:", cmm == {"a": 1, "b": 2, "c": 3}, cmm.setdefault("d", 4), cmm.pop("b"))
cmm.clear()
print("CustomMutableMapping cleared:", len(cmm), cmm == {})


class CustomSequence(abc.Sequence):
    def __init__(self, items):
        self._items = list(items)

    def __getitem__(self, idx):
        return self._items[idx]

    def __len__(self):
        return len(self._items)


cs = CustomSequence([10, 20, 30, 20])
print(
    "CustomSequence mixins:",
    list(cs),
    list(reversed(cs)),
    20 in cs,
    99 in cs,
    cs.index(20),
    cs.count(20),
)


class CustomSet(abc.Set):
    def __init__(self, items):
        self._items = set(items)

    def __contains__(self, x):
        return x in self._items

    def __iter__(self):
        return iter(self._items)

    def __len__(self):
        return len(self._items)


s1 = CustomSet([1, 2])
s2 = CustomSet([2, 3])
print(
    "CustomSet mixins:",
    s1 == {1, 2},
    {1, 2} == s1,
    s1 < {1, 2, 3},
    sorted(s1 & s2),
    sorted(s1 | s2),
    sorted(s1 - s2),
    s1.isdisjoint({4, 5}),
)
