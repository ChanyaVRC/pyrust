# functools.singledispatch (PEP 443) — added in #2599.  Single-dispatch
# generic functions: the implementation is chosen by the type of the
# first positional argument, resolved through that type's MRO.

import functools
from functools import singledispatch


# ── explicit @register(cls) form ─────────────────────────────────────

@singledispatch
def process(arg):
    return f"generic: {arg}"


@process.register(int)
def _(arg):
    return f"int: {arg}"


@process.register(str)
def _(arg):
    return f"str: {arg}"


print(process(42))      # int: 42
print(process("hi"))    # str: hi
print(process(3.14))    # generic: 3.14 (no float impl → falls back)
print(process([1, 2]))  # generic: [1, 2]


# ── annotation form (@g.register, type from first param annotation) ──

@functools.singledispatch
def g(x):
    return "default"


@g.register
def _(x: int):
    return "int"


@g.register
def _(x: list):
    return "list"


print(g("s"), g(1), g([]))   # default int list


# ── MRO-based dispatch ───────────────────────────────────────────────

class Base:
    pass


class Derived(Base):
    pass


@singledispatch
def describe(x):
    return "object"


@describe.register(Base)
def _(x):
    return "base"


print(describe(Derived()))   # base — Derived has no own impl, inherits Base
print(describe(Base()))      # base
print(describe(42))          # object


# ── register(cls, func) two-argument form ────────────────────────────

def handle_bool(x):
    return "bool!"


describe.register(bool, handle_bool)
print(describe(True))        # bool!  (bool is a subclass of int, but its
                             # own impl wins over int/object)


# ── dispatch() and registry introspection ───────────────────────────

print(process.dispatch(int).__name__ == "_")        # True
print(process.dispatch(float) is process.dispatch(complex))  # True (both → generic)
print(g.__name__)                                    # g (update_wrapper)
print(sorted(g.registry.keys(), key=lambda k: k.__name__))


# ── empty-call error parity ──────────────────────────────────────────

try:
    process()
except TypeError as e:
    print("TypeError:", e)


# ── singledispatch caches dispatch results across calls ──────────────

@singledispatch
def count_kind(x):
    return "other"


@count_kind.register(int)
def _(x):
    return "number"


print(count_kind(1), count_kind(2), count_kind("x"), count_kind(3))
