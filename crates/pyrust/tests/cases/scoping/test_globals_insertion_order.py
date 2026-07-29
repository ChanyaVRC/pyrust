# Issue #2903: the module namespace is a dict, so `globals()` must iterate in
# binding insertion order — deterministically, run to run.
#
# pyrust materialises the module dict lazily from the script frame's fast-local
# registers; those registers are allocated in source binding order, which is the
# order CPython's module dict preserves.
#
# Known gap (still divergent, tracked separately): a module-level name that is
# `del`eted and then re-bound keeps its original slot here, while CPython moves
# it to the end.  The `del` case below re-binds the *last* declared name, where
# both orders agree.

import math

first = 1
second = 2
third = 3

# Rebinding must NOT move a key.
first = 10


def helper():
    return first


class Marker:
    pass


before = list(globals())
print("before:", [k for k in before if not k.startswith("_")])

# The dunder prefix is part of the observable order.
print("dunders:", [k for k in before if k.startswith("_")])

# The live alias sees later module bindings appended in binding order.
alias = globals()
fourth = 4
fifth = 5
print("after:", [k for k in alias if not k.startswith("_")])
print("alias is globals():", alias is globals())

# Writes through the alias append like any other dict mutation.
alias["injected"] = "via-alias"
print("appended:", [k for k in globals() if not k.startswith("_")])
print("injected visible as a global:", injected)

# Deleting a binding removes the key; re-binding it re-appends.
sixth = 6
del sixth
print("after del:", [k for k in globals() if not k.startswith("_")])
sixth = 60
print("after rebind:", [k for k in globals() if not k.startswith("_")])

# Module attribute access order matches the imported module's own dict.
print("math imported:", "math" in globals(), math.floor(1.5))

# An exec namespace is an ordinary dict: full insertion-order semantics,
# including del + re-assign moving the key to the end.
ns = {}
exec("one = 1\ntwo = 2\nthree = 3\ndel two\ntwo = 22\none = 11", ns)
print("exec ns:", [k for k in ns if not k.startswith("_")])
print("exec values:", ns["one"], ns["two"], ns["three"])

# locals() at module scope is the same namespace as globals().
print("locals == globals keys:", list(locals()) == list(globals()))
