# Parity fixture for issue #2946:
# 1. collections.deque.__copy__ & collections.defaultdict.__copy__
# 2. Built-in __reduce__ / __reduce_ex__ CPython 3.12.3 matrix
# 3. copy.copy / copy.deepcopy reject types.MappingProxyType
# 4. object.__getstate__ (with __slots__) & hiding internal backing attrs from __dict__
# 5. deque subclass attribute assignment & SlottedDeque
# 6. deepcopy memo identity for instance __dict__ proxies

import collections
import copy
import types


def catch_error(fn):
    try:
        fn()
    except Exception as exc:
        return (type(exc).__name__, str(exc))
    return ("OK", None)


# ── 1. deque.__copy__ and defaultdict.__copy__ ────────────────────────────────

print("hasattr deque.__copy__:", hasattr(collections.deque, "__copy__"))
print("dir deque.__copy__:", "__copy__" in dir(collections.deque))
print("hasattr defaultdict.__copy__:", hasattr(collections.defaultdict, "__copy__"))
print("dir defaultdict.__copy__:", "__copy__" in dir(collections.defaultdict))


class SubDeque(collections.deque):
    pass


class SubDD(collections.defaultdict):
    pass


shared = [1, 2]
d = collections.deque([shared, 3], maxlen=5)
d_c1 = d.__copy__()
d_c2 = collections.deque.__copy__(d)
d_c3 = copy.copy(d)
for label, c in (("bound", d_c1), ("unbound", d_c2), ("copy", d_c3)):
    assert type(c) is collections.deque, label
    assert c is not d, label
    assert list(c) == [[1, 2], 3], label
    assert c.maxlen == 5, label
    assert c[0] is shared, label
    c.append(9)
    assert len(d) == 2, label

sd = SubDeque([shared, 4], maxlen=4)
sd.x = 99
for label, c in (
    ("sub-bound", sd.__copy__()),
    ("sub-unbound", collections.deque.__copy__(sd)),
    ("sub-copy", copy.copy(sd)),
):
    assert type(c) is SubDeque, label
    assert list(c) == [[1, 2], 4], label
    assert c.maxlen == 4, label
    assert c[0] is shared, label
    assert not hasattr(c, "x"), label
    assert c.__dict__ == {}, label

dd = collections.defaultdict(list, a=shared)
for label, c in (
    ("dd-bound", dd.__copy__()),
    ("dd-unbound", collections.defaultdict.__copy__(dd)),
    ("dd-copy", copy.copy(dd)),
):
    assert type(c) is collections.defaultdict, label
    assert c is not dd, label
    assert dict(c) == {"a": [1, 2]}, label
    assert c.default_factory is list, label
    assert c["a"] is shared, label
    c["b"] = 1
    assert "b" not in dd, label

sdd = SubDD(list, a=shared)
sdd.tag = "hi"
for label, c in (
    ("sdd-bound", sdd.__copy__()),
    ("sdd-unbound", collections.defaultdict.__copy__(sdd)),
    ("sdd-copy", copy.copy(sdd)),
):
    assert type(c) is SubDD, label
    assert dict(c) == {"a": [1, 2]}, label
    assert c.default_factory is list, label
    assert c["a"] is shared, label
    assert not hasattr(c, "tag"), label
    assert c.__dict__ == {}, label

# Invalid receivers / arity / kwargs for __copy__
print("deque.__copy__():", catch_error(lambda: collections.deque.__copy__()))
print("deque.__copy__(1):", catch_error(lambda: collections.deque.__copy__(1)))
print("deque.__copy__(d, 1):", catch_error(lambda: collections.deque.__copy__(d, 1)))
print("deque.__copy__(d, x=1):", catch_error(lambda: collections.deque.__copy__(d, x=1)))
print("d.__copy__(1):", catch_error(lambda: d.__copy__(1)))
print("d.__copy__(x=1):", catch_error(lambda: d.__copy__(x=1)))
print("dd.__copy__():", catch_error(lambda: collections.defaultdict.__copy__()))
print("dd.__copy__({}):", catch_error(lambda: collections.defaultdict.__copy__({})))
print("dd.__copy__(dd, 1):", catch_error(lambda: collections.defaultdict.__copy__(dd, 1)))
print("dd.__copy__(dd, x=1):", catch_error(lambda: collections.defaultdict.__copy__(dd, x=1)))
print("dd_inst.__copy__(1):", catch_error(lambda: dd.__copy__(1)))
print("dd_inst.__copy__(x=1):", catch_error(lambda: dd.__copy__(x=1)))


# ── 2. Built-in __reduce__ / __reduce_ex__ matrix ─────────────────────────────

# Exact types that reject __reduce__() and __reduce_ex__(proto < 2)
for sample in (
    {"a": 1},
    [1, 2],
    (1, 2),
    42,
    1.5,
    True,
    1 + 2j,
    "abc",
    b"abc",
    None,
):
    tname = type(sample).__name__
    print(f"reduce {tname} None:", catch_error(lambda s=sample: s.__reduce__()))
    print(f"reduce {tname} 0:", catch_error(lambda s=sample: s.__reduce_ex__(0)))
    print(f"reduce {tname} 1:", catch_error(lambda s=sample: s.__reduce_ex__(1)))
    for proto in (2, 4):
        red = sample.__reduce_ex__(proto)
        assert type(red) is tuple and len(red) == 5, (tname, proto, red)
        fn, rargs, state, listiter, dictiter = red
        rebuilt = fn(*rargs)
        if listiter is not None:
            for item in listiter:
                rebuilt.append(item)
        if dictiter is not None:
            for k, v in dictiter:
                rebuilt[k] = v
        assert type(rebuilt) is type(sample), (tname, proto, type(rebuilt))
        assert rebuilt == sample, (tname, proto, rebuilt, sample)
        assert state is None, (tname, proto, state)

# Unbound calls on dict / list / object
print("dict.__reduce__({}):", catch_error(lambda: dict.__reduce__({})))
print("list.__reduce__([]):", catch_error(lambda: list.__reduce__([])))
print("object.__reduce__({}):", catch_error(lambda: object.__reduce__({})))

# Exact types that succeed for ALL protocols
for sample in (
    {1, 2},
    frozenset({1, 2}),
    bytearray(),
    bytearray(b"abc"),
    collections.deque([1, 2]),
    collections.deque([1, 2], maxlen=5),
    collections.defaultdict(None, a=1),
    collections.defaultdict(int, a=1),
):
    tname = type(sample).__name__
    for proto in (None, 0, 1, 2, 3, 4):
        red = sample.__reduce__() if proto is None else sample.__reduce_ex__(proto)
        assert type(red) is tuple and len(red) >= 2, (tname, proto, red)
        fn, rargs = red[0], red[1]
        rebuilt = fn(*rargs)
        if len(red) >= 4 and red[3] is not None:
            for item in red[3]:
                rebuilt.append(item)
        if len(red) >= 5 and red[4] is not None:
            for k, v in red[4]:
                rebuilt[k] = v
        assert type(rebuilt) is type(sample), (tname, proto, type(rebuilt))
        assert rebuilt == sample, (tname, proto, rebuilt, sample)

# Verify bytearray proto < 3 vs proto >= 3 tuple args
ba_red_2 = bytearray(b"abc").__reduce_ex__(2)
ba_red_3 = bytearray(b"abc").__reduce_ex__(3)
print("bytearray proto 2 args:", ba_red_2[1])
print("bytearray proto 3 args:", ba_red_3[1])
print("bytearray empty args:", bytearray().__reduce__()[1])

# range / Ellipsis / NotImplemented
print("range reduce:", range(1, 10, 2).__reduce__())
print("ellipsis reduce:", Ellipsis.__reduce__())
print("notimplemented reduce:", NotImplemented.__reduce__())

# Subclasses of dict / list / int / str / tuple / set / frozenset / bytearray / deque / defaultdict
class MyDict(dict):
    pass


class MyList(list):
    pass


class MyInt(int):
    pass


class MyStr(str):
    pass


class MyTuple(tuple):
    pass


class MySet(set):
    pass


class MyFrozenset(frozenset):
    pass


class MyBytearray(bytearray):
    pass


md = MyDict(a=1)
md.tag = "x"
md_r0 = md.__reduce__()
assert type(md_r0) is tuple and len(md_r0) == 3 and md_r0[2] == {"tag": "x"}
md_rebuilt = md_r0[0](*md_r0[1])
assert type(md_rebuilt) is MyDict and dict(md_rebuilt) == {"a": 1}
md_r2 = md.__reduce_ex__(2)
assert type(md_r2) is tuple and len(md_r2) == 5 and md_r2[2] == {"tag": "x"}
md2 = md_r2[0](*md_r2[1])
for k, v in md_r2[4]:
    md2[k] = v
assert type(md2) is MyDict and dict(md2) == {"a": 1}

ml = MyList([1, 2])
ml.tag = "y"
ml_r0 = ml.__reduce__()
assert type(ml_r0) is tuple and len(ml_r0) == 3 and ml_r0[2] == {"tag": "y"}
ml_rebuilt = ml_r0[0](*ml_r0[1])
assert type(ml_rebuilt) is MyList and list(ml_rebuilt) == [1, 2]
ml_r2 = ml.__reduce_ex__(2)
assert type(ml_r2) is tuple and len(ml_r2) == 5 and ml_r2[2] == {"tag": "y"}
ml2 = ml_r2[0](*ml_r2[1])
for item in ml_r2[3]:
    ml2.append(item)
assert type(ml2) is MyList and list(ml2) == [1, 2]

# Empty subclass reduce has no state (2-tuple for proto < 2)
assert len(MyDict().__reduce__()) == 2
assert len(MyList().__reduce__()) == 2

sd_red = sd.__reduce__()
assert sd_red[0] is SubDeque and sd_red[1] == ((), 4) and sd_red[2] == {"x": 99}
assert sd.__reduce_ex__(4)[2] == {"x": 99}

sdd_red = sdd.__reduce__()
assert sdd_red[0] is SubDD and sdd_red[1] == (list,) and sdd_red[2] is None
assert sdd.__reduce_ex__(4)[0] is SubDD

# ── 3. MappingProxyType copy / deepcopy / reduce ──────────────────────────────

mp_dict = types.MappingProxyType({"a": 1})


class DummyForProxy:
    val = 1


mp_cls = DummyForProxy.__dict__
for label, mp in (("dict-proxy", mp_dict), ("cls-proxy", mp_cls)):
    print(f"copy {label}:", catch_error(lambda m=mp: copy.copy(m)))
    print(f"deepcopy {label}:", catch_error(lambda m=mp: copy.deepcopy(m)))
    print(f"reduce {label}:", catch_error(lambda m=mp: m.__reduce__()))
    for proto in (0, 1, 2, 4):
        print(
            f"reduce_ex({proto}) {label}:",
            catch_error(lambda m=mp, p=proto: m.__reduce_ex__(p)),
        )


# ── 4. object.__getstate__ and hiding internal attrs ──────────────────────────

print("hasattr object.__getstate__:", hasattr(object, "__getstate__"))
print("dir object.__getstate__:", "__getstate__" in dir(object))
print("object().__getstate__():", object().__getstate__())
print("(42).__getstate__():", (42).__getstate__())
print("{}.__getstate__():", {}.__getstate__())

print("getstate no self:", catch_error(lambda: object.__getstate__()))
print("getstate pos1:", catch_error(lambda: object.__getstate__(object(), 1)))
print("getstate kw:", catch_error(lambda: object.__getstate__(object(), x=1)))
print("getstate bound pos1:", catch_error(lambda: (1).__getstate__(1)))
print("getstate bound kw:", catch_error(lambda: (1).__getstate__(x=1)))


class Plain:
    pass


p = Plain()
print("Plain empty getstate:", p.__getstate__())
p.a = 1
p.b = [2]
st = p.__getstate__()
print("Plain state:", st)
assert type(st) is dict and st == {"a": 1, "b": [2]}


class SlottedOnly:
    __slots__ = ("x", "y")


so = SlottedOnly()
print("SlottedOnly empty:", so.__getstate__())
so.x = 10
print("SlottedOnly x=10:", so.__getstate__())
so_copy = copy.copy(so)
so_deep = copy.deepcopy(so)
print("SlottedOnly copies:", so_copy.x, hasattr(so_copy, "y"), so_deep.x, hasattr(so_deep, "y"))


class SlottedWithDict:
    __slots__ = ("x", "__dict__")


swd = SlottedWithDict()
print("SlottedWithDict empty:", swd.__getstate__())
swd.d = 20
print("SlottedWithDict dict only:", swd.__getstate__())
swd.x = 10
print("SlottedWithDict both:", swd.__getstate__())
del swd.d
print("SlottedWithDict slot only:", swd.__getstate__())

# No internal backing attrs leaked in __dict__, vars(), dir(), __getstate__()
for name, obj in (
    ("MyDict", MyDict(a=1)),
    ("MyList", MyList([1])),
    ("MyInt", MyInt(5)),
    ("MyStr", MyStr("hi")),
    ("MyTuple", MyTuple((1, 2))),
    ("MySet", MySet([1])),
    ("MyFrozenset", MyFrozenset([1])),
    ("MyBytearray", MyBytearray(b"ab")),
    ("Counter", collections.Counter("aab")),
    ("OrderedDict", collections.OrderedDict(a=1)),
    ("SubDeque", SubDeque([1, 2], maxlen=4)),
    ("SubDD", SubDD(list, a=[1])),
):
    assert obj.__dict__ == {}, (name, obj.__dict__)
    assert vars(obj) == {}, (name, vars(obj))
    assert obj.__getstate__() is None, (name, obj.__getstate__())
    dnames = dir(obj)
    assert "__builtin_data__" not in dnames, (name, dnames)
    assert "_items" not in dnames, (name, dnames)

# Exact deque and exact defaultdict have no __dict__
for name, obj in (
    ("deque", collections.deque([1])),
    ("defaultdict", collections.defaultdict(int, a=1)),
):
    assert not hasattr(obj, "__dict__"), name
    print(f"vars({name}):", catch_error(lambda o=obj: vars(o)))

# defaultdict.default_factory stays out of SubDD.__dict__
sdd2 = SubDD(int)
sdd2.default_factory = list
assert sdd2.default_factory is list
assert sdd2.__dict__ == {}
del sdd2.default_factory
assert sdd2.default_factory is None
assert sdd2.__dict__ == {}


# ── 5. deque subclass attribute assignment & SlottedDeque ─────────────────────

dq = SubDeque([1, [2]], maxlen=4)
dq.x = 99
assert dq.x == 99
assert dq.__dict__ == {"x": 99}
assert vars(dq) == {"x": 99}
assert dq.__getstate__() == {"x": 99}

dq_deep = copy.deepcopy(dq)
assert type(dq_deep) is SubDeque
assert list(dq_deep) == [1, [2]]
assert dq_deep.maxlen == 4
assert dq_deep.x == 99
assert dq_deep[1] is not dq[1]

del dq.x
assert dq.__dict__ == {}
print("del missing SubDeque attr:", catch_error(lambda: delattr(dq, "x")))
print("SubDeque maxlen write:", catch_error(lambda: setattr(dq, "maxlen", 10)))
print("SubDeque maxlen del:", catch_error(lambda: delattr(dq, "maxlen")))
print("exact deque write:", catch_error(lambda: setattr(collections.deque([1]), "x", 1)))
print("exact deque maxlen write:", catch_error(lambda: setattr(collections.deque([1]), "maxlen", 10)))


class SlottedDeque(collections.deque):
    __slots__ = ("a",)


sld = SlottedDeque([10, 20], maxlen=3)
sld.a = 7
assert sld.a == 7
assert not hasattr(sld, "__dict__")
print("SlottedDeque bad attr:", catch_error(lambda: setattr(sld, "b", 8)))
print("SlottedDeque getstate:", sld.__getstate__())
sld_shallow = copy.copy(sld)
assert type(sld_shallow) is SlottedDeque and list(sld_shallow) == [10, 20] and sld_shallow.maxlen == 3
assert not hasattr(sld_shallow, "a")
sld_deep = copy.deepcopy(sld)
assert type(sld_deep) is SlottedDeque and list(sld_deep) == [10, 20] and sld_deep.maxlen == 3
assert sld_deep.a == 7
assert not hasattr(sld_deep, "__dict__")


# ── 6. deepcopy memo identity for __dict__ proxies ────────────────────────────


class Holder:
    pass


h = Holder()
assert h.__dict__ is h.__dict__
assert id(h.__dict__) == id(h.__dict__)
h.__dict__["self_d"] = h.__dict__
d_copy = copy.deepcopy(h.__dict__)
assert type(d_copy) is dict
assert d_copy is not h.__dict__
assert d_copy["self_d"] is d_copy

h3 = Holder()
h3.a = h3.__dict__
h3.b = h3.__dict__
h3_copy = copy.deepcopy(h3)
assert h3_copy.a is h3_copy.b
assert h3_copy.a is not h3_copy.__dict__
assert h3_copy.a is not h3.__dict__
assert h3_copy.a["a"] is h3_copy.a
assert h3_copy.a["b"] is h3_copy.a

print("all #2946 checks ok")
