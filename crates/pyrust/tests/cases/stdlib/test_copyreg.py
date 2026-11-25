# Parity fixture for `copyreg` (issue #3208) and its integration with
# `pickle` and `copy`.

import copy
import copyreg
import pickle


def catch_error(fn):
    try:
        fn()
    except Exception as exc:
        return (type(exc).__name__, str(exc))
    return ("OK", None)


# ── 1. Module metadata & default dispatch_table ───────────────────────────────

print("all:", copyreg.__all__)
print("shared dispatch_table:", copy.dispatch_table is copyreg.dispatch_table)
print("complex in dispatch_table:", complex in copyreg.dispatch_table)
print("union in dispatch_table:", type(int | str) in copyreg.dispatch_table)

# pickle_complex & pickle_union helpers
fn_c, args_c = copyreg.pickle_complex(3 + 4j)
print("pickle_complex:", fn_c is complex, args_c, fn_c(*args_c))

u = int | str
fn_u, args_u = copyreg.pickle_union(u)
print("pickle_union:", fn_u(*args_u) == u)

# ── 2. constructor() and pickle() validation ──────────────────────────────────

copyreg.constructor(list)
copyreg.constructor(lambda x: x)
print("constructor non-callable:", catch_error(lambda: copyreg.constructor(123)))
print(
    "pickle non-callable reducer:",
    catch_error(lambda: copyreg.pickle(int, "not-callable")),
)
print(
    "pickle non-callable ctor:",
    catch_error(lambda: copyreg.pickle(int, lambda x: x, 42)),
)


# ── 3. copyreg.pickle() integration with pickle AND copy ──────────────────────

class Box:
    def __init__(self, value, tag="default"):
        self.value = value
        self.tag = tag

    def __eq__(self, other):
        return isinstance(other, Box) and (self.value, self.tag) == (
            other.value,
            other.tag,
        )


def rebuild_box(value, tag):
    return Box(value, f"{tag}+rebuilt")


def reduce_box(box):
    return (rebuild_box, (box.value, box.tag))


copyreg.pickle(Box, reduce_box, rebuild_box)
try:
    b = Box([1, 2], "orig")
    for proto in range(pickle.HIGHEST_PROTOCOL + 1):
        roundtripped = pickle.loads(pickle.dumps(b, protocol=proto))
        assert isinstance(roundtripped, Box)
        assert roundtripped.value == [1, 2]
        assert roundtripped.tag == "orig+rebuilt"
    print("pickle via copyreg.pickle: ok")

    b_shallow = copy.copy(b)
    assert isinstance(b_shallow, Box)
    assert b_shallow.value is b.value
    assert b_shallow.tag == "orig+rebuilt"

    b_deep = copy.deepcopy(b)
    assert isinstance(b_deep, Box)
    assert b_deep.value == [1, 2]
    assert b_deep.value is not b.value
    assert b_deep.tag == "orig+rebuilt"
    print("copy via copyreg.pickle: ok")
finally:
    del copyreg.dispatch_table[Box]


# Precedence: __copy__ / __deepcopy__ beat copyreg.dispatch_table,
# while copyreg.dispatch_table beats __reduce__.
class PrecedenceCheck:
    def __init__(self, v):
        self.v = v

    def __reduce__(self):
        return (PrecedenceCheck, (self.v + "->reduce",))


copyreg.pickle(
    PrecedenceCheck,
    lambda o: (PrecedenceCheck, (o.v + "->dispatch",)),
)
try:
    pc = PrecedenceCheck("a")
    print("precedence copy:", copy.copy(pc).v)
    print("precedence deepcopy:", copy.deepcopy(pc).v)
    print("precedence pickle:", pickle.loads(pickle.dumps(pc)).v)
finally:
    del copyreg.dispatch_table[PrecedenceCheck]


class WithCopyOverride:
    def __init__(self, v):
        self.v = v

    def __copy__(self):
        return WithCopyOverride(self.v + "->copy")

    def __deepcopy__(self, memo):
        return WithCopyOverride(self.v + "->deepcopy")


copyreg.pickle(
    WithCopyOverride,
    lambda o: (WithCopyOverride, (o.v + "->dispatch",)),
)
try:
    wc = WithCopyOverride("x")
    print("override copy:", copy.copy(wc).v)
    print("override deepcopy:", copy.deepcopy(wc).v)
finally:
    del copyreg.dispatch_table[WithCopyOverride]


# ── 4. Extension registry & pickle EXT1 / EXT2 / EXT4 ─────────────────────────

class ExtSample:
    pass


try:
    copyreg.add_extension(__name__, "ExtSample", 240)
    # Redundant registration is benign
    copyreg.add_extension(__name__, "ExtSample", 240)
    print("ext registered:", copyreg._extension_registry[(__name__, "ExtSample")])
    print("ext inverted:", copyreg._inverted_registry[240])

    # Duplicate key with different code
    print(
        "dup key:",
        catch_error(lambda: copyreg.add_extension(__name__, "ExtSample", 241)),
    )
    # Duplicate code with different key
    print(
        "dup code:",
        catch_error(lambda: copyreg.add_extension(__name__, "OtherSample", 240)),
    )
    # Out-of-range codes
    print("code 0:", catch_error(lambda: copyreg.add_extension("m", "n", 0)))
    print("code -1:", catch_error(lambda: copyreg.add_extension("m", "n", -1)))
    print(
        "code too large:",
        catch_error(lambda: copyreg.add_extension("m", "n", 0x80000000)),
    )

    # Pickle round-trip uses EXT1 and populates _extension_cache
    for proto in (2, 3, 4, 5):
        data = pickle.dumps(ExtSample, protocol=proto)
        assert pickle.EXT1 in data
        assert pickle.loads(data) is ExtSample
    print("ext cache populated:", 240 in copyreg._extension_cache)
    copyreg.clear_extension_cache()
    print("ext cache cleared:", len(copyreg._extension_cache))

    # Wrong code on remove_extension
    print(
        "bad remove:",
        catch_error(lambda: copyreg.remove_extension(__name__, "ExtSample", 241)),
    )
finally:
    if (__name__, "ExtSample") in copyreg._extension_registry:
        copyreg.remove_extension(__name__, "ExtSample", 240)

print(
    "removed cleanly:",
    (__name__, "ExtSample") not in copyreg._extension_registry,
    240 not in copyreg._inverted_registry,
)

# EXT2 (code > 255) and EXT4 (code > 65535)
for label, code, opcode in (
    ("EXT2", 300, pickle.EXT2),
    ("EXT4", 70000, pickle.EXT4),
):
    copyreg.add_extension(__name__, "ExtSample", code)
    try:
        data = pickle.dumps(ExtSample, protocol=4)
        assert opcode in data
        assert pickle.loads(data) is ExtSample
        print(f"ext {label}: ok")
    finally:
        copyreg.remove_extension(__name__, "ExtSample", code)


# ── 5. _reconstructor, __newobj__, __newobj_ex__ ──────────────────────────────

class PlainObj:
    def __init__(self, x=0):
        self.x = x


rec_plain = copyreg._reconstructor(PlainObj, object, None)
print("reconstructor plain:", type(rec_plain) is PlainObj, hasattr(rec_plain, "x"))


class MyInt(int):
    pass


rec_int = copyreg._reconstructor(MyInt, int, 42)
print("reconstructor int:", type(rec_int) is MyInt, rec_int == 42)


class CustomNew:
    def __new__(cls, a, b=10, *, kw="k"):
        obj = super().__new__(cls)
        obj.a = a
        obj.b = b
        obj.kw = kw
        return obj


n1 = copyreg.__newobj__(CustomNew, 1, 2)
print("newobj:", type(n1) is CustomNew, n1.a, n1.b, n1.kw)

n2 = copyreg.__newobj_ex__(CustomNew, (3,), {"b": 4, "kw": "z"})
print("newobj_ex:", type(n2) is CustomNew, n2.a, n2.b, n2.kw)


# ── 6. _slotnames() ───────────────────────────────────────────────────────────

class NoSlots:
    pass


class SingleSlot:
    __slots__ = "alpha"


class BaseSlots:
    __slots__ = ("a", "__priv", "__dunder__", "__dict__", "__weakref__")


class ChildSlots(BaseSlots):
    __slots__ = ("b", "__child_priv")


class _LeadingUnderscore:
    __slots__ = ("__secret",)


class ___AllUnderscore:
    __slots__ = ("__keep",)


print("slotnames NoSlots:", copyreg._slotnames(NoSlots))
print("slotnames SingleSlot:", copyreg._slotnames(SingleSlot))
print("slotnames ChildSlots:", copyreg._slotnames(ChildSlots))
print("slotnames cached:", ChildSlots.__dict__["__slotnames__"] == copyreg._slotnames(ChildSlots))
print("slotnames _LeadingUnderscore:", copyreg._slotnames(_LeadingUnderscore))
print("slotnames ___AllUnderscore:", copyreg._slotnames(___AllUnderscore))
print("slotnames int:", copyreg._slotnames(int))

print("copyreg tests ok")
