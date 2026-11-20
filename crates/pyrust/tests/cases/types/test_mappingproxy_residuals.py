import copy
from collections import OrderedDict
from types import MappingProxyType


def expect_error(fn, exc_type, msg=None):
    try:
        fn()
    except exc_type as e:
        if msg is not None:
            assert str(e) == msg, f"expected {msg!r}, got {str(e)!r}"
        return e
    raise AssertionError(f"expected {exc_type.__name__}")


# 1. Rich comparison (== / !=) & | delegation across dict-, class-, and object-backed proxies
p = MappingProxyType({"a": 1})
od = OrderedDict(a=1)
assert p == od
assert od == p
assert not (p != od)
assert not (od != p)
assert [p] == [od]
assert [od] == [p]
assert p in [od]
assert od in [p]
assert [od].index(p) == 0
assert [od].count(p) == 1

# Dict-backed proxy vs OrderedDict is order-insensitive; OD-backed proxy vs OD is order-sensitive
p_ab = MappingProxyType({"a": 1, "b": 2})
od_ba = OrderedDict([("b", 2), ("a", 1)])
assert p_ab == od_ba
assert od_ba == p_ab
od_ab = OrderedDict([("a", 1), ("b", 2)])
assert not (MappingProxyType(od_ab) == od_ba)
assert MappingProxyType(od_ab) != od_ba

# | delegation returns OrderedDict for dict-backed and class-backed proxies
m_po = p | od
assert type(m_po).__name__ == "OrderedDict"
assert m_po == OrderedDict(a=1)
m_op = od | MappingProxyType({"b": 2})
assert type(m_op).__name__ == "OrderedDict"
assert list(m_op.items()) == [("a", 1), ("b", 2)]


class C1:
    x = 1


assert C1.__dict__ == OrderedDict(C1.__dict__)
assert OrderedDict(C1.__dict__) == C1.__dict__
m_co = C1.__dict__ | OrderedDict(z=9)
assert type(m_co).__name__ == "OrderedDict"
assert m_co["x"] == 1 and m_co["z"] == 9

# Nested proxies
nested = MappingProxyType(MappingProxyType({"a": 1}))
assert nested == od
assert type(nested | OrderedDict(b=2)).__name__ == "OrderedDict"


# 2. hash(p) delegation
expect_error(lambda: hash(MappingProxyType({"a": 1})), TypeError, "unhashable type: 'dict'")
expect_error(lambda: hash(C1.__dict__), TypeError, "unhashable type: 'dict'")
expect_error(lambda: {MappingProxyType({}): 1}, TypeError, "unhashable type: 'dict'")
expect_error(lambda: {MappingProxyType({})}, TypeError, "unhashable type: 'dict'")
expect_error(lambda: hash((MappingProxyType({}),)), TypeError, "unhashable type: 'dict'")


class HashableDict(dict):
    def __hash__(self):
        return 42


h = HashableDict(a=1)
ph = MappingProxyType(h)
assert hash(ph) == 42
assert hash(MappingProxyType(ph)) == 42
d_keyed = {ph: "ok"}
assert d_keyed[ph] == "ok"
assert d_keyed[h] == "ok"
assert ph in {ph}


# 3. Ordering (< / <= / > / >=) delegation and reflected operator preservation
expect_error(
    lambda: MappingProxyType({"a": 1}) < 3,
    TypeError,
    "'<' not supported between instances of 'dict' and 'int'",
)
expect_error(
    lambda: MappingProxyType({"a": 1}) <= 3,
    TypeError,
    "'<=' not supported between instances of 'dict' and 'int'",
)
expect_error(
    lambda: MappingProxyType({"a": 1}) > 3,
    TypeError,
    "'>' not supported between instances of 'dict' and 'int'",
)
expect_error(
    lambda: MappingProxyType({"a": 1}) >= 3,
    TypeError,
    "'>=' not supported between instances of 'dict' and 'int'",
)
expect_error(
    lambda: MappingProxyType({"a": 1}) < {"a": 1},
    TypeError,
    "'<' not supported between instances of 'dict' and 'dict'",
)
expect_error(
    lambda: MappingProxyType({"a": 1}) < MappingProxyType({"a": 1, "b": 2}),
    TypeError,
    "'>' not supported between instances of 'dict' and 'dict'",
)
expect_error(
    lambda: MappingProxyType({"a": 1}) <= MappingProxyType({"a": 1, "b": 2}),
    TypeError,
    "'>=' not supported between instances of 'dict' and 'dict'",
)
expect_error(
    lambda: MappingProxyType({"a": 1}) > MappingProxyType({"a": 1, "b": 2}),
    TypeError,
    "'<' not supported between instances of 'dict' and 'dict'",
)
expect_error(
    lambda: MappingProxyType({"a": 1}) >= MappingProxyType({"a": 1, "b": 2}),
    TypeError,
    "'<=' not supported between instances of 'dict' and 'dict'",
)
expect_error(
    lambda: 3 < MappingProxyType({"a": 1}),
    TypeError,
    "'>' not supported between instances of 'dict' and 'int'",
)
expect_error(
    lambda: C1.__dict__ < 3,
    TypeError,
    "'<' not supported between instances of 'dict' and 'int'",
)
expect_error(
    lambda: 3 < C1.__dict__,
    TypeError,
    "'>' not supported between instances of 'dict' and 'int'",
)


class OrdMap(dict):
    def __init__(self, n):
        super().__init__(n=n)
        self.n = n

    def __lt__(self, other):
        if isinstance(other, OrdMap):
            return self.n < other.n
        return NotImplemented

    def __le__(self, other):
        if isinstance(other, OrdMap):
            return self.n <= other.n
        return NotImplemented

    def __gt__(self, other):
        if isinstance(other, OrdMap):
            return self.n > other.n
        return NotImplemented

    def __ge__(self, other):
        if isinstance(other, OrdMap):
            return self.n >= other.n
        return NotImplemented


om1 = OrdMap(1)
om2 = OrdMap(2)
assert MappingProxyType(om1) < MappingProxyType(om2)
assert MappingProxyType(om1) <= MappingProxyType(om2)
assert MappingProxyType(om2) > MappingProxyType(om1)
assert MappingProxyType(om2) >= MappingProxyType(om1)
assert MappingProxyType(om1) < om2
assert om1 < MappingProxyType(om2)


# 4. Dunder method surface on instances and MappingProxyType class
assert p.__or__({"b": 2}) == {"a": 1, "b": 2}
assert p.__ror__({"b": 2}) == {"b": 2, "a": 1}
assert list(p.__ror__({"b": 2}).keys()) == ["b", "a"]
expect_error(
    lambda: p.__or__(1),
    TypeError,
    "unsupported operand type(s) for |: 'dict' and 'int'",
)
expect_error(
    lambda: p.__ror__(1),
    TypeError,
    "unsupported operand type(s) for |: 'int' and 'dict'",
)
expect_error(
    lambda: p.__ior__({"b": 2}),
    TypeError,
    "'|=' is not supported by mappingproxy; use '|' instead",
)
expect_error(
    lambda: MappingProxyType.__ior__(p, {"b": 2}),
    TypeError,
    "'|=' is not supported by mappingproxy; use '|' instead",
)
assert p.__eq__({"a": 1}) is True
assert p.__eq__(1) is False
assert p.__ne__(1) is True
expect_error(
    lambda: p.__lt__(1),
    TypeError,
    "'<' not supported between instances of 'dict' and 'int'",
)
expect_error(lambda: p.__hash__(), TypeError, "unhashable type: 'dict'")
assert ph.__hash__() == 42
assert p.__getitem__("a") == 1
assert p.__len__() == 1
assert p.__contains__("a") is True
assert list(p.__iter__()) == ["a"]
assert list(p.__reversed__()) == ["a"]
assert p.__repr__() == "mappingproxy({'a': 1})"
assert p.__str__() == "{'a': 1}"

# GenericAlias / __class_getitem__
ga1 = MappingProxyType[str, int]
assert ga1.__origin__ is MappingProxyType
assert ga1.__args__ == (str, int)
ga2 = p.__class_getitem__(int)
assert ga2.__origin__ is MappingProxyType
assert ga2.__args__ == (int,)

# Unbound class-level descriptor calls
assert list(MappingProxyType.keys(p)) == ["a"]
assert list(MappingProxyType.values(p)) == [1]
assert list(MappingProxyType.items(p)) == [("a", 1)]
assert MappingProxyType.get(p, "a") == 1
assert MappingProxyType.get(p, "missing", 99) == 99
assert MappingProxyType.copy(p) == {"a": 1}
assert type(MappingProxyType.copy(p)) is dict
assert MappingProxyType.__getitem__(p, "a") == 1
assert MappingProxyType.__len__(p) == 1
assert MappingProxyType.__contains__(p, "a") is True
assert list(MappingProxyType.__iter__(p)) == ["a"]
assert list(MappingProxyType.__reversed__(p)) == ["a"]
assert MappingProxyType.__or__(p, {"b": 2}) == {"a": 1, "b": 2}
assert MappingProxyType.__ror__(p, {"b": 2}) == {"b": 2, "a": 1}
assert MappingProxyType.__eq__(p, {"a": 1}) is True
assert MappingProxyType.__repr__(p) == "mappingproxy({'a': 1})"
assert MappingProxyType.__str__(p) == "{'a': 1}"
assert hasattr(MappingProxyType, "keys")
assert hasattr(MappingProxyType, "__getitem__")
assert hasattr(MappingProxyType, "__or__")
assert hasattr(MappingProxyType, "__class_getitem__")


# 5. copy.copy / copy.deepcopy / __reduce__ / __reduce_ex__
for proxy in (p, C1.__dict__, MappingProxyType(od)):
    expect_error(lambda pr=proxy: copy.copy(pr), TypeError, "cannot pickle 'mappingproxy' object")
    expect_error(
        lambda pr=proxy: copy.deepcopy(pr), TypeError, "cannot pickle 'mappingproxy' object"
    )
    expect_error(
        lambda pr=proxy: pr.__reduce__(), TypeError, "cannot pickle 'mappingproxy' object"
    )
    expect_error(
        lambda pr=proxy: pr.__reduce_ex__(4), TypeError, "cannot pickle 'mappingproxy' object"
    )

cp = p.copy()
assert type(cp) is dict and cp == {"a": 1}


# 6. Unhashable-key errors on dict-backed and class-backed proxies
for target in (p, C1.__dict__):
    expect_error(lambda t=target: t[[]], TypeError, "unhashable type: 'list'")
    expect_error(lambda t=target: t.get([]), TypeError, "unhashable type: 'list'")
    expect_error(lambda t=target: [] in t, TypeError, "unhashable type: 'list'")
    expect_error(lambda t=target: t[{}], TypeError, "unhashable type: 'dict'")
    expect_error(lambda t=target: t.get({}), TypeError, "unhashable type: 'dict'")
    expect_error(lambda t=target: {} in t, TypeError, "unhashable type: 'dict'")
    expect_error(lambda t=target: t[set()], TypeError, "unhashable type: 'set'")
    expect_error(lambda t=target: t.get(set()), TypeError, "unhashable type: 'set'")
    expect_error(lambda t=target: set() in t, TypeError, "unhashable type: 'set'")

ke_str = expect_error(lambda: p["zz"], KeyError)
assert ke_str.args == ("zz",)
ke_int = expect_error(lambda: C1.__dict__[1], KeyError)
assert ke_int.args == (1,)
assert C1.__dict__.get(1, "fallback") == "fallback"
assert (1 in C1.__dict__) is False


# 7. dir(p) matches CPython 3.12's exact 38 names
expected_dir = [
    "__class__",
    "__class_getitem__",
    "__contains__",
    "__delattr__",
    "__dir__",
    "__doc__",
    "__eq__",
    "__format__",
    "__ge__",
    "__getattribute__",
    "__getitem__",
    "__getstate__",
    "__gt__",
    "__hash__",
    "__init__",
    "__init_subclass__",
    "__ior__",
    "__iter__",
    "__le__",
    "__len__",
    "__lt__",
    "__ne__",
    "__new__",
    "__or__",
    "__reduce__",
    "__reduce_ex__",
    "__repr__",
    "__reversed__",
    "__ror__",
    "__setattr__",
    "__sizeof__",
    "__str__",
    "__subclasshook__",
    "copy",
    "get",
    "items",
    "keys",
    "values",
]
assert len(expected_dir) == 38
for target in (p, C1.__dict__, MappingProxyType(od)):
    assert dir(target) == expected_dir
    assert all(hasattr(target, n) for n in dir(target))


# 8. View .mapping liveness & owner preservation
class LiveClass:
    x = 1


mk = LiveClass.__dict__.keys().mapping
mv = LiveClass.__dict__.values().mapping
mi = LiveClass.__dict__.items().mapping
LiveClass.y = 2
for m in (mk, mv, mi):
    assert "y" in m
    assert m["y"] == 2
    assert len(m) == len(LiveClass.__dict__)
    assert not hasattr(m, "__setitem__")
    try:
        m["z"] = 1
        raise AssertionError("expected TypeError")
    except TypeError as e:
        assert str(e) == "'mappingproxy' object does not support item assignment"

od_live = OrderedDict(a=1)
m_od_k = od_live.keys().mapping
m_od_v = od_live.values().mapping
m_od_i = od_live.items().mapping
m_mp_k = MappingProxyType(od_live).keys().mapping
m_mp_v = MappingProxyType(od_live).values().mapping
m_mp_i = MappingProxyType(od_live).items().mapping
od_live["b"] = 2
od_rev = OrderedDict([("b", 2), ("a", 1)])
for m in (m_od_k, m_od_v, m_od_i, m_mp_k, m_mp_v, m_mp_i):
    assert "b" in m
    assert list(m) == ["a", "b"]
    assert not (m == od_rev)
    merged = m | OrderedDict(c=3)
    assert type(merged).__name__ == "OrderedDict"
    assert list(merged.items()) == [("a", 1), ("b", 2), ("c", 3)]
    try:
        m["z"] = 1
        raise AssertionError("expected TypeError")
    except TypeError as e:
        assert str(e) == "'mappingproxy' object does not support item assignment"

# Plain dict .mapping regression check
plain = {"a": 1}
pm = plain.keys().mapping
plain["b"] = 2
assert "b" in pm and pm["b"] == 2

print("mappingproxy residuals ok")
