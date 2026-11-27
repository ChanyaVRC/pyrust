# Parity fixture for the 100% native `copyreg` module (issue #3459): every
# public/private helper of CPython 3.12.3's `Lib/copyreg.py`, its live module
# tables, and the `pickle` / `copy` integrations that consume them.

import copy
import copyreg
import pickle


def err(fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except Exception as exc:
        return (type(exc).__name__, str(exc))
    return ("OK", None)


# ── 1. Module surface and live tables ────────────────────────────────────────

print("all:", copyreg.__all__)
for name in (
    "pickle", "constructor", "add_extension", "remove_extension",
    "clear_extension_cache", "pickle_complex", "pickle_union", "dispatch_table",
    "_reconstructor", "_reduce_ex", "__newobj__", "__newobj_ex__", "_slotnames",
    "_extension_registry", "_inverted_registry", "_extension_cache",
    "_HEAPTYPE", "_new_type",
):
    print("has", name, hasattr(copyreg, name))
print("tables are dicts:", [
    type(t).__name__ for t in (
        copyreg.dispatch_table, copyreg._extension_registry,
        copyreg._inverted_registry, copyreg._extension_cache,
    )
])
print("HEAPTYPE:", copyreg._HEAPTYPE, "new_type:", copyreg._new_type.__name__)
print("shared with copy:", copy.dispatch_table is copyreg.dispatch_table)
print("complex registered:", copyreg.dispatch_table[complex] is copyreg.pickle_complex)
print("union registered:", copyreg.dispatch_table[type(int | str)] is copyreg.pickle_union)
print("default order:", list(copyreg.dispatch_table)[:2] == [complex, type(int | str)])
print("registries start empty:", (
    copyreg._extension_registry, copyreg._inverted_registry, copyreg._extension_cache,
))

# ── 2. pickle_complex / pickle_union ─────────────────────────────────────────

print("pickle_complex:", copyreg.pickle_complex(3 + 4j))
print("pickle_complex rt:", pickle.loads(pickle.dumps(1.5 - 2j)) == 1.5 - 2j)
fn, a = copyreg.pickle_union(int | str)
print("pickle_union:", fn(*a) == (int | str), a[0].__name__, a[1])
for proto in range(pickle.HIGHEST_PROTOCOL + 1):
    print("union rt", proto, pickle.loads(pickle.dumps(int | str, proto)) == (int | str))
print("copy complex:", copy.copy(2 + 3j), copy.deepcopy(2 + 3j))

# ── 3. constructor() and pickle() ────────────────────────────────────────────

print("constructor ok:", copyreg.constructor(len), copyreg.constructor(int))
print("constructor 5:", err(copyreg.constructor, 5))
print("constructor none:", err(copyreg.constructor, None))
print("constructor missing:", err(copyreg.constructor))
print("constructor extra:", err(copyreg.constructor, len, len))
print("constructor kw:", err(copyreg.constructor, object=len))


class Pt:
    def __init__(self, x, y):
        self.x = x
        self.y = y


def reduce_pt(p):
    return Pt, (p.x * 10, p.y * 10)


print("pickle bad reducer:", err(copyreg.pickle, Pt, 42))
print("pickle bad reducer None:", err(copyreg.pickle, Pt, None))
print("pickle bad ctor:", err(copyreg.pickle, Pt, reduce_pt, "no"))
print("pickle missing:", err(copyreg.pickle, Pt))
print("pickle too many:", err(copyreg.pickle, Pt, reduce_pt, Pt, 1))
print("pickle unexpected kw:", err(copyreg.pickle, Pt, reduce_pt, bogus=1))
print("pickle no side effect:", Pt in copyreg.dispatch_table)

print("pickle ret:", copyreg.pickle(Pt, reduce_pt))
print("registered:", copyreg.dispatch_table[Pt] is reduce_pt)
try:
    for proto in range(pickle.HIGHEST_PROTOCOL + 1):
        p = pickle.loads(pickle.dumps(Pt(1, 2), proto))
        print("pt pickle", proto, p.x, p.y)
    c = copy.copy(Pt(3, 4))
    print("pt copy:", c.x, c.y)
    d = copy.deepcopy(Pt(5, 6))
    print("pt deepcopy:", d.x, d.y)
    # Re-registration replaces the reducer; ctor argument is validated only.
    print("pickle ctor ok:", copyreg.pickle(Pt, lambda p: (Pt, (0, 0)), Pt))
    print("replaced:", copy.copy(Pt(9, 9)).x)
finally:
    del copyreg.dispatch_table[Pt]
print("cleaned:", Pt not in copyreg.dispatch_table)

# The module table is looked up live: a rebound / patched table is honoured.
sentinel = {}
saved = copyreg.dispatch_table
copyreg.dispatch_table = sentinel
try:
    copyreg.pickle(Pt, reduce_pt)
    print("rebound table:", Pt in sentinel, Pt in saved)
finally:
    copyreg.dispatch_table = saved
print("restored:", copyreg.dispatch_table is saved)

# ── 4. Extension registry ────────────────────────────────────────────────────

print("add ret:", copyreg.add_extension("mod_a", "Name", 1000))
print("registry:", copyreg._extension_registry, copyreg._inverted_registry)
print("add idempotent:", err(copyreg.add_extension, "mod_a", "Name", 1000))
print("add idempotent str code:", err(copyreg.add_extension, "mod_a", "Name", "1000"))
print("add idempotent float code:", err(copyreg.add_extension, "mod_a", "Name", 1000.9))
print("add dup key:", err(copyreg.add_extension, "mod_a", "Name", 1001))
print("add dup code:", err(copyreg.add_extension, "mod_b", "Other", 1000))
print("add bool code:", err(copyreg.add_extension, "mod_b", "T", True))
print("add 0:", err(copyreg.add_extension, "m", "n", 0))
print("add -1:", err(copyreg.add_extension, "m", "n", -1))
print("add 2**31:", err(copyreg.add_extension, "m", "n", 2**31))
print("add 2**31-1:", err(copyreg.add_extension, "m", "big", 2**31 - 1))
print("add 2**100:", err(copyreg.add_extension, "m", "n", 2**100))
print("add bad code:", err(copyreg.add_extension, "m", "n", "xyz"))
print("add None code:", err(copyreg.add_extension, "m", "n", None))
print("add missing:", err(copyreg.add_extension, "m", "n"))
print("add kw:", err(copyreg.add_extension, module="kw_mod", name="kw", code=77))
print("after failures:", sorted(copyreg._extension_registry.items(), key=repr))
print("inverted:", sorted(copyreg._inverted_registry.items(), key=lambda kv: kv[0]))

copyreg._extension_cache[1000] = "cached"
copyreg._extension_cache[77] = "cached77"
print("remove ret:", copyreg.remove_extension("kw_mod", "kw", 77))
print("cache entry dropped:", 77 in copyreg._extension_cache, 1000 in copyreg._extension_cache)
print("remove nonexistent:", err(copyreg.remove_extension, "nope", "nope", 5))
print("remove wrong code:", err(copyreg.remove_extension, "mod_a", "Name", 1001))
print("remove wrong key:", err(copyreg.remove_extension, "mod_a", "Other", 1000))
print("remove twice:", err(copyreg.remove_extension, "kw_mod", "kw", 77))
print("remove str code:", err(copyreg.remove_extension, "mod_a", "Name", "1000"))
print("remove missing:", err(copyreg.remove_extension, "mod_a"))
print("still present:", ("mod_a", "Name") in copyreg._extension_registry)
print("clear ret:", copyreg.clear_extension_cache())
print("cache cleared:", copyreg._extension_cache)
print("clear twice:", copyreg.clear_extension_cache(), err(copyreg.clear_extension_cache, 1))
print("remove ok:", copyreg.remove_extension("mod_a", "Name", 1000))
print("remove big:", copyreg.remove_extension("m", "big", 2**31 - 1))
print("empty again:", copyreg._extension_registry, copyreg._inverted_registry)

# int-like code is normalised through int(); non-int keys are accepted.
class Idx:
    def __int__(self):
        return 4242


copyreg.add_extension("tup", ("a", 1), Idx())
print("int() code:", copyreg._extension_registry, copyreg._inverted_registry)
print("dup via int():", err(copyreg.add_extension, "tup", ("a", 1), 4242))
print("remove:", err(copyreg.remove_extension, "tup", ("a", 1), 4242))

# Registered extension codes drive the pickle EXT opcodes on real classes.


class Ext:
    pass


for code, opcode in ((200, pickle.EXT1), (300, pickle.EXT2), (70000, pickle.EXT4)):
    copyreg.add_extension(__name__, "Ext", code)
    try:
        for proto in (2, 3, 4, 5):
            data = pickle.dumps(Ext, protocol=proto)
            assert opcode in data, (code, proto)
            assert pickle.loads(data) is Ext
        print("ext", code, "cache:", code in copyreg._extension_cache)
        copyreg.clear_extension_cache()
    finally:
        copyreg.remove_extension(__name__, "Ext", code)
print("ext final:", copyreg._extension_registry, copyreg._inverted_registry)
print("ext cache final:", copyreg._extension_cache)

# ── 5. _reconstructor ────────────────────────────────────────────────────────


class Plain:
    pass


class WithInit:
    def __init__(self):
        raise RuntimeError("must not run")


class MyInt(int):
    pass


class MyStr(str):
    pass


class MyTuple(tuple):
    pass


class MyList(list):
    pass


class MyDict(dict):
    pass


class MyFloat(float):
    pass


o = copyreg._reconstructor(Plain, object, None)
print("rec object:", type(o) is Plain, vars(o))
o = copyreg._reconstructor(WithInit, object, None)
print("rec object skips init:", type(o) is WithInit)
for cls, base, state in (
    (MyInt, int, 42), (MyStr, str, "hello"), (MyTuple, tuple, (1, 2, 3)),
    (MyList, list, [1, 2]), (MyDict, dict, {"a": 1}), (MyFloat, float, 2.5),
):
    o = copyreg._reconstructor(cls, base, state)
    print("rec", cls.__name__, type(o) is cls, o)
print("rec identity:", copyreg._reconstructor(Plain, object, None) is
      copyreg._reconstructor(Plain, object, None))
print("rec missing:", err(copyreg._reconstructor, Plain, object))
print("rec extra:", err(copyreg._reconstructor, Plain, object, None, None))
print("rec bad cls:", err(copyreg._reconstructor, 5, object, None))
print("rec same as reduce:", copyreg._reconstructor is Plain().__reduce__()[0])

# ── 6. __newobj__ / __newobj_ex__ ────────────────────────────────────────────


class New:
    def __new__(cls, a, b=10, *, kw="k"):
        self = super().__new__(cls)
        self.a, self.b, self.kw = a, b, kw
        return self


class Keyword(New):
    def __new__(cls, *, only):
        self = object.__new__(cls)
        self.only = only
        return self


n = copyreg.__newobj__(New, 1)
print("newobj:", type(n) is New, n.a, n.b, n.kw)
n = copyreg.__newobj__(New, 1, 2)
print("newobj 2:", n.a, n.b, n.kw)
print("newobj no args:", err(copyreg.__newobj__, New))
print("newobj none:", err(copyreg.__newobj__))
print("newobj kw:", err(copyreg.__newobj__, New, 1, kw="x"))
print("newobj plain:", type(copyreg.__newobj__(Plain)) is Plain)
print("newobj int:", copyreg.__newobj__(MyInt, 7), type(copyreg.__newobj__(MyInt, 7)).__name__)
print("newobj str:", copyreg.__newobj__(MyStr, "s"))
print("newobj plain extra:", err(copyreg.__newobj__, Plain, 1))
print("newobj list:", copyreg.__newobj__(MyList), copyreg.__newobj__(MyList, [1]))
print("newobj same as reduce_ex2:", copyreg.__newobj__ is Plain().__reduce_ex__(2)[0])

n = copyreg.__newobj_ex__(New, (3,), {})
print("newobj_ex empty kw:", n.a, n.b, n.kw)
n = copyreg.__newobj_ex__(New, (3,), {"b": 4, "kw": "z"})
print("newobj_ex:", n.a, n.b, n.kw)
n = copyreg.__newobj_ex__(New, [3, 5], {"kw": "z"})
print("newobj_ex list args:", n.a, n.b, n.kw)
n = copyreg.__newobj_ex__(Keyword, (), {"only": 9})
print("newobj_ex kwonly:", type(n) is Keyword, n.only)
print("newobj_ex bad kw:", err(copyreg.__newobj_ex__, Keyword, (), {"nope": 1}))
print("newobj_ex missing:", err(copyreg.__newobj_ex__, New, ()))
print("newobj_ex bad args:", err(copyreg.__newobj_ex__, New, 5, {}))
print("newobj_ex int:", type(copyreg.__newobj_ex__(MyInt, (3,), {})).__name__)
print("newobj_ex plain:", type(copyreg.__newobj_ex__(Plain, (), {})).__name__)

# ── 7. _slotnames ────────────────────────────────────────────────────────────


class NoSlots:
    pass


class OneSlot:
    __slots__ = "only"


class Multi:
    __slots__ = ["a", "b"]


class WithDict:
    __slots__ = ("__dict__", "__weakref__", "z")


class Priv:
    __slots__ = ("__p", "__dunder__", "plain", "___triple")


class _Under:
    __slots__ = ("__x",)


class ___Only:
    __slots__ = ("__y",)


class ChildA(Multi):
    __slots__ = ("c", "__d")


class ChildB(ChildA):
    pass


class Mixed(NoSlots, OneSlot):
    pass


class EmptySlots:
    __slots__ = ()


class DictSlots:
    __slots__ = {"d1": "doc", "d2": None}


for cls in (NoSlots, OneSlot, Multi, WithDict, Priv, _Under, ___Only, ChildA,
            ChildB, Mixed, EmptySlots, DictSlots, int, str, object, tuple):
    first = copyreg._slotnames(cls)
    print("slotnames", cls.__name__, first, first is copyreg._slotnames(cls))
print("cached in class dict:", [c.__name__ for c in (
    NoSlots, OneSlot, Multi, Priv, ChildA, ChildB, EmptySlots)
    if "__slotnames__" in c.__dict__])
print("cached value:", ChildA.__dict__["__slotnames__"], Multi.__slotnames__)
print("child sees cache of own class only:", "__slotnames__" in ChildB.__dict__)

# A pre-seeded cache wins over recomputation.
class Seeded:
    __slots__ = ("real",)
    __slotnames__ = ["fake"]


print("seeded:", copyreg._slotnames(Seeded))
print("slotnames missing:", err(copyreg._slotnames))
print("slotnames no __dict__:", err(copyreg._slotnames, 5))
print("slotnames via pickle:", pickle.loads(pickle.dumps(ChildA.__new__(ChildA), 2)).__class__.__name__)


class Slotted:
    __slots__ = ("p", "q")

    def __init__(self):
        self.p = 1
        self.q = 2


for proto in (2, 3, 4, 5):
    s = pickle.loads(pickle.dumps(Slotted(), proto))
    print("slotted rt", proto, s.p, s.q)
print("slotted copy:", copy.copy(Slotted()).p, copy.deepcopy(Slotted()).q)

# ── 8. _reduce_ex (protocols 0 and 1) ────────────────────────────────────────


def show(obj, proto):
    try:
        r = copyreg._reduce_ex(obj, proto)
    except Exception as exc:
        return (type(exc).__name__, str(exc))
    fn, args = r[0], r[1]
    cls, base, state = args
    return (
        fn is copyreg._reconstructor,
        cls.__name__,
        base.__name__,
        state,
        r[2] if len(r) > 2 else "<no dict>",
    )


class Inst:
    pass


class WithState:
    def __init__(self):
        self.a = 1
        self.b = [2]


class Empty:
    pass


class GetState:
    def __init__(self):
        self.a = 1

    def __getstate__(self):
        return {"custom": True}


class GetStateFalsy:
    def __getstate__(self):
        return {}


class SlotsOnly:
    __slots__ = ("a",)

    def __init__(self):
        self.a = 1


class SlotsGetState:
    __slots__ = ("a",)

    def __getstate__(self):
        return ("s", 1)


class DictSub(dict):
    pass


for proto in (0, 1):
    print("reduce_ex Inst", proto, show(Inst(), proto))
    print("reduce_ex WithState", proto, show(WithState(), proto))
    print("reduce_ex GetState", proto, show(GetState(), proto))
    print("reduce_ex GetStateFalsy", proto, show(GetStateFalsy(), proto))
    print("reduce_ex SlotsOnly", proto, show(SlotsOnly(), proto))
    print("reduce_ex SlotsGetState", proto, show(SlotsGetState(), proto))
    print("reduce_ex MyInt", proto, show(MyInt(5), proto))
    print("reduce_ex MyStr", proto, show(MyStr("x"), proto))
    print("reduce_ex MyTuple", proto, show(MyTuple((1, 2)), proto))
    print("reduce_ex MyFloat", proto, show(MyFloat(1.5), proto))
    print("reduce_ex MyList", proto, show(MyList([1, 2]), proto))
    print("reduce_ex DictSub", proto, show(DictSub(a=1), proto))
print("reduce_ex proto 2:", show(Inst(), 2))
print("reduce_ex proto 5:", show(Inst(), 5))
print("reduce_ex missing:", err(copyreg._reduce_ex, Inst()))
print("reduce_ex builtin int:", show(5, 1))
print("reduce_ex builtin list:", show([1], 0))

# Users reach `_reduce_ex` through object.__reduce_ex__ for protocols 0 and 1.
for proto in (0, 1):
    for factory in (Inst, WithState, lambda: MyInt(9), lambda: MyStr("t"),
                    lambda: MyTuple((1, 2)), lambda: MyFloat(0.5)):
        obj = factory()
        r = obj.__reduce_ex__(proto)
        print("object.__reduce_ex__", proto, type(obj).__name__, len(r),
              r[0] is copyreg._reconstructor)

# ── 9. pickle / copy round trips through the reduction machinery ─────────────

objs = [
    Inst(), WithState(), GetState(), MyInt(12), MyStr("hello"),
    MyTuple((1, 2, 3)), MyFloat(3.25), MyList([1, 2]), DictSub(a=1, b=2),
]
for proto in range(pickle.HIGHEST_PROTOCOL + 1):
    row = []
    for obj in objs:
        try:
            back = pickle.loads(pickle.dumps(obj, proto))
        except Exception as exc:
            row.append(type(exc).__name__)
            continue
        row.append((type(back).__name__, getattr(back, "__dict__", None)
                    if not isinstance(back, (int, str, tuple, float, list, dict))
                    else back))
    print("rt", proto, row)
for obj in objs:
    c, d = copy.copy(obj), copy.deepcopy(obj)
    print("copy", type(obj).__name__, type(c) is type(obj), type(d) is type(obj))

for proto in (0, 1):
    for bad in (SlotsOnly(),):
        print("slots rt", proto, err(pickle.dumps, bad, proto))

print("copyreg native ok")
