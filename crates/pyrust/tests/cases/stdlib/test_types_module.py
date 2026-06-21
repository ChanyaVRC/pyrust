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
