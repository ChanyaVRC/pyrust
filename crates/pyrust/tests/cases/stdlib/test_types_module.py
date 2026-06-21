import types

# ── Type-object constants ──────────────────────────────────────────────────
# These reference the interpreter's internal type singletons, so identity with
# `type(...)` of a live object of that kind holds.

print(type(None) is types.NoneType)
print(types.NoneType.__name__)

print(type(lambda: 0) is types.FunctionType)
print(types.FunctionType.__name__)


def _fn():
    return 0


print(type(_fn) is types.FunctionType)

# LambdaType is an alias of FunctionType (same object).
print(types.LambdaType is types.FunctionType)

# Built-in functions/methods share BuiltinFunctionType.
print(type(len) is types.BuiltinFunctionType)
print(type(print) is types.BuiltinFunctionType)
print(types.BuiltinMethodType is types.BuiltinFunctionType)

# ── MappingProxyType ───────────────────────────────────────────────────────
mp = types.MappingProxyType({"a": 1, "b": 2})
print(repr(mp))
print(mp["a"])
print(len(mp))
print("a" in mp)
print(list(mp.keys()))
print(mp == {"a": 1, "b": 2})

# Read-only: assignment raises TypeError.
try:
    mp["c"] = 3
except TypeError:
    print("readonly")

# Wrong argument type raises TypeError.
try:
    types.MappingProxyType([1, 2])
except TypeError as e:
    print(type(e).__name__, str(e))

# Missing argument raises TypeError.
try:
    types.MappingProxyType()
except TypeError:
    print("missing-arg")

# MappingProxyType IS the runtime mappingproxy type (identity), not a
# lookalike: `type(<proxy>) is types.MappingProxyType` and it is the same
# object as `type(<class>.__dict__)`.
print(type(mp) is types.MappingProxyType)
print(type(int.__dict__) is types.MappingProxyType)
print(type(types.MappingProxyType({})) is types.MappingProxyType)

# `mapping` is positional-or-keyword.
print(repr(types.MappingProxyType(mapping={"k": 1})))

# A second argument is rejected with the at-most-1 wording.
try:
    types.MappingProxyType({}, {})
except TypeError as e:
    print(e)

# ── GenericAlias (callable constructor) ────────────────────────────────────
print(types.GenericAlias(list, int))
print(types.GenericAlias(list, int).__args__)
print(types.GenericAlias(dict, (str, int)))
print(type(types.GenericAlias(list, (int,))) is types.GenericAlias)
print(types.GenericAlias(dict, (str, types.GenericAlias(list, int))))

for _bad in (
    "types.GenericAlias(list)",
    "types.GenericAlias()",
    "types.GenericAlias(list, int, str)",
    "types.GenericAlias(list, int, foo=1)",
):
    try:
        eval(_bad)
    except TypeError as _e:
        print(type(_e).__name__, _e)

# ── SimpleNamespace ────────────────────────────────────────────────────────
ns = types.SimpleNamespace(x=1, y=2)
print(repr(ns))
print(ns.x, ns.y)

ns.z = 3
print(ns.z)
print(repr(ns))

print(types.SimpleNamespace())
print(types.SimpleNamespace(x=1) == types.SimpleNamespace(x=1))
print(types.SimpleNamespace(x=1) == types.SimpleNamespace(x=2))
print(types.SimpleNamespace(b=2, a=1))

# Unhashable (defines __eq__).
try:
    hash(types.SimpleNamespace())
except TypeError:
    print("unhashable")

# Empty namespace then mutate.
empty = types.SimpleNamespace()
empty.first = "hello"
print(empty.first)
print(repr(empty))


# Subclass repr uses the subclass name (base reprs as `namespace`).
class _NS(types.SimpleNamespace):
    pass


print(repr(_NS(x=1)))
print(_NS(x=1) == types.SimpleNamespace(x=1))

# ── Runtime-object type constants (issue #2777) ────────────────────────────
# These reference the interpreter's runtime type for generators, coroutines,
# async generators, bound methods, unions, Ellipsis, and NotImplemented, so
# identity with `type(...)` of a live object of that kind holds.
from typing import get_origin, get_args  # noqa: E402


def _gen():
    yield 1


async def _coro():
    pass


async def _agen():
    yield 1


_g = _gen()
_c = _coro()
_ag = _agen()

print(type(_g) is types.GeneratorType)
print(types.GeneratorType.__name__)
print(type(_c) is types.CoroutineType)
print(types.CoroutineType.__name__)
print(type(_ag) is types.AsyncGeneratorType)
print(types.AsyncGeneratorType.__name__)


class _Meth:
    def m(self):
        return 0


print(type(_Meth().m) is types.MethodType)
print(types.MethodType.__name__)
print(repr(types.MethodType))

# PEP 604 unions.
print(type(int | str) is types.UnionType)
print(types.UnionType.__name__)

# EllipsisType / NotImplementedType are the real primitive classes.
print(type(...) is types.EllipsisType)
print(types.EllipsisType.__name__)
print(repr(types.EllipsisType))
print(type(NotImplemented) is types.NotImplementedType)
print(types.NotImplementedType.__name__)
print(repr(types.NotImplementedType))

# isinstance against the runtime type constants (not just `is`/identity).
print(isinstance(_g, types.GeneratorType))
print(isinstance(_c, types.CoroutineType))
print(isinstance(_ag, types.AsyncGeneratorType))
print(isinstance(_Meth().m, types.MethodType))
print(isinstance(int | str, types.UnionType))
print(isinstance(..., types.EllipsisType))
print(isinstance(NotImplemented, types.NotImplementedType))
# A generator is not a coroutine and vice versa.
print(isinstance(_g, types.CoroutineType))
print(isinstance(_c, types.GeneratorType))

# Secondary effect: get_origin / get_args on a runtime union.
print(get_origin(int | str) is types.UnionType)
print(get_args(int | str))
print(get_origin(int) is None)

_c.close()
