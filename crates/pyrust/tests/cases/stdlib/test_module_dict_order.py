# Issue #2918: a module's `__dict__` is a Python dict, so it must iterate in a
# stable, insertion-defined order.  `PyModule::attrs` was a `HashMap`, so
# `list(vars(math))` differed run to run.
#
# WHAT THIS FIXTURE PINS (both interpreters agree on all of it):
#   * the five module-object slots CPython puts at the head of a built-in
#     module's dict, in CPython's order;
#   * `vars(m)` and `m.__dict__` agreeing as listings;
#   * dict mutation ordering semantics on a module namespace — rebinding keeps a
#     name's position, `del` shifts the survivors rather than swapping the tail
#     into the hole, and re-binding a deleted name appends it;
#   * that a re-synthesised `__dict__` is identical to the previous one.
#
# WHAT THIS FIXTURE DOES NOT PIN:
#   * the order of the module's *own* names.  pyrust lists them in its module
#     body's declaration order while CPython lists them in its C method-table
#     order (`math` is alphabetical there), so the two differ after the head.
#     Only determinism is required, and that is asserted cross-process by
#     `tests/module_dict_order_determinism.rs` — a single parity run cannot
#     observe a hash seed changing.
#   * `len(vars(m))` or the full member set, which would tie the fixture to one
#     CPython version's module surface.

import math
import sys

SLOTS = ["__name__", "__doc__", "__package__", "__loader__", "__spec__"]

# CPython initialises a built-in module's dict with these slots before running
# the module's method table, so they lead the listing in this exact order.
print(list(math.__dict__)[:5] == SLOTS)
print(list(vars(sys))[:5] == SLOTS)

# `vars(m)` is defined as `m.__dict__`; they must not be two different views.
print(list(vars(math)) == list(math.__dict__))
print(all(name in vars(math) for name in SLOTS))

# Two independent reads agree (a freshly synthesised dict is not re-shuffled).
print(list(math.__dict__) == list(math.__dict__))

# The listing is a permutation of itself under sorting, and sorting genuinely
# changes it -- i.e. the namespace is in insertion order, not key order.
names = list(math.__dict__)
print(sorted(names) == sorted(set(names)) and len(names) == len(set(names)))
print(names != sorted(names))

# Dict ordering semantics on a live built-in module namespace.  These are the
# assertions that separate `shift_remove` from `swap_remove`: deleting one name
# must shift the survivors up rather than swap the tail entry into the hole.
before = list(math.__dict__)
saved = math.pi
del math.pi
print(list(math.__dict__) == [n for n in before if n != "pi"])

# Re-binding a deleted name appends it, exactly like `dict`.
math.pi = saved
print(list(math.__dict__)[-1] == "pi")
print(math.pi == saved)

# Rebinding an existing name keeps its position.
after_rebind = list(math.__dict__)
math.e = math.e
print(list(math.__dict__) == after_rebind)

# A brand-new attribute lands at the end, and deleting it restores the listing.
math.pyrust_probe_2918 = 7
print(list(math.__dict__)[-1] == "pyrust_probe_2918")
del math.pyrust_probe_2918
print(list(math.__dict__) == after_rebind)

# Rebinding one of the five slots is also a rebind, not an append: the key is
# already in the dict, so it keeps its head position and only its value changes.
saved_name = math.__name__
math.__name__ = "renamed"
print(list(math.__dict__)[:5] == SLOTS, math.__dict__["__name__"] == "renamed")
math.__name__ = saved_name
print(list(math.__dict__)[:5] == SLOTS, math.__name__ == "math")

# A source-backed module keeps the same five-slot head.  Its own names follow in
# execution order; the tail past the head holds extra interpreter bookkeeping
# keys whose order is not pinned here.
import _module_dict_order_source as source

print(list(vars(source))[:5] == SLOTS)
print([name for name in vars(source) if not name.startswith("__")])
print(list(vars(source)) == list(source.__dict__))
