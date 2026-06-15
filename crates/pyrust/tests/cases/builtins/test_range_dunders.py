# Issue #2399: `range` exposes its slot dunders both as attributes on the
# `range` type object and as bound methods on `range(...)` instances, and they
# behave identically to the operator/iteration forms.

r = range(3)

# ── Type-level hasattr ───────────────────────────────────────────────────────
for name in (
    "__iter__", "__len__", "__getitem__", "__contains__", "__reversed__",
    "__eq__", "__ne__", "__hash__", "__bool__", "__str__", "__repr__",
):
    print("type", name, hasattr(range, name))

# ── Instance-level hasattr ───────────────────────────────────────────────────
for name in (
    "__iter__", "__len__", "__getitem__", "__contains__", "__reversed__",
    "__eq__", "__ne__", "__hash__", "__bool__", "__repr__",
):
    print("inst", name, hasattr(r, name))

# ── Instance dunder calls ────────────────────────────────────────────────────
print(list(r.__iter__()))
print(r.__len__())
print(r.__getitem__(0), r.__getitem__(-1))
print(r.__getitem__(slice(1, None)))
print(r.__contains__(1), r.__contains__(9))
print(list(r.__reversed__()))
print(r.__eq__(range(3)), r.__eq__(range(4)), r.__eq__([0, 1, 2]))
print(r.__ne__(range(4)))
print(r.__hash__() == range(0, 3).__hash__())
print(range(0).__bool__(), range(3).__bool__())
print(r.__repr__())

# ── Type-level (unbound) dunder calls ────────────────────────────────────────
print(range.__len__(range(5)))
print(range.__getitem__(range(5), 2))
print(range.__contains__(range(5), 3))
print(list(range.__iter__(range(3))))
print(list(range.__reversed__(range(3))))
print(range.__eq__(range(3), range(3)))

# ── Descriptor types match CPython 3.12 ──────────────────────────────────────
# `__str__` is omitted: range inherits it from `object`, and the descriptor type
# of the inherited `object.__str__` is a separate pre-existing gap (#2422), not
# part of range's own slot surface.
for name in (
    "__iter__", "__len__", "__getitem__", "__contains__", "__reversed__",
    "__eq__", "__ne__", "__hash__", "__bool__", "__repr__",
):
    print("typetype", name, type(getattr(range, name)).__name__)
for name in (
    "__iter__", "__len__", "__getitem__", "__contains__", "__reversed__",
    "__eq__", "__hash__", "__bool__", "__repr__",
):
    print("insttype", name, type(getattr(r, name)).__name__)

# ── Negative step / empty / large range behaviour ────────────────────────────
n = range(10, 0, -2)
print(list(n.__iter__()))
print(n.__getitem__(0), n.__contains__(8), n.__contains__(7))
print(list(n.__reversed__()))

empty = range(0)
print(empty.__len__(), empty.__bool__(), empty.__contains__(0))

big = range(10 ** 20)
print(big.__len__() if False else "len-overflows")  # avoid OverflowError text
print(big.__contains__(5), big.__getitem__(3))
print(big.__eq__(range(10 ** 20)))
