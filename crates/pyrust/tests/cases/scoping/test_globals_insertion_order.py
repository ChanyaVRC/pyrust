# Issues #2903 & #2919: the module namespace is a dict, so `globals()` and
# imported module `__dict__`s must iterate in exact CPython binding insertion
# order — deterministically, run to run.
#
# Covers:
#   - live re-bind keeping its position;
#   - `del NAME` then re-bind moving the name to the end (both before and after
#     `globals()` is called, both at module scope and via `global`);
#   - names created only via a `global` write inside a function landing at
#     their first binding point;
#   - imported filesystem module dunder prefix order (`__file__` and
#     `__cached__` before `__builtins__`, lazy `__annotations__` when the
#     module has no module-level annotations, eager `__annotations__` after
#     `__builtins__` when it does).

import math

first = 1
second = 2
third = 3

# Rebinding a live name must NOT move it.
first = 10

# Pre-globals() del + rebind of a non-last name must move it to the end.
del first
first = 100


def helper():
    return first


class Marker:
    pass


# Pre-globals() name created ONLY via a `global` write inside a function must
# land at its first binding point, not before declared module slots.
def set_global_only():
    global g_only
    g_only = 42


# Pre-globals() del + rebind via `global` inside a function must move the name
# to the end as well.
def bounce_second():
    global second
    del second
    second = 200


before_global_only = 4
set_global_only()
bounce_second()
after_global_only = 5

before = list(globals())
print("before:", [k for k in before if not k.startswith("_")])
print("values:", first, second, third, g_only, before_global_only, after_global_only)

# The __main__ dunder prefix is part of the observable order.
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

# Post-globals() deleting a non-last binding removes it; re-binding appends it.
sixth = 6
del third
print("after del non-last:", [k for k in globals() if not k.startswith("_")])
third = 300
print("after rebind non-last:", [k for k in globals() if not k.startswith("_")])

# Post-globals() global-write-only and del+rebind via global.
def set_and_bounce_post():
    global g_only_post, fourth
    g_only_post = 99
    del fourth
    fourth = 400

set_and_bounce_post()
after_post = 7
print("after post global ops:", [k for k in globals() if not k.startswith("_")])
print("post values:", third, fourth, g_only_post, after_post)

# Module attribute access order matches the imported module's own dict.
print("math imported:", "math" in globals(), math.floor(1.5))

# Imported plain .py module: __file__ and __cached__ before __builtins__, and
# NO __annotations__ key until accessed.
import _globals_order_plain_2919 as plain_mod

print("plain initial:", list(plain_mod.__dict__))
print("plain has key before access:", "__annotations__" in plain_mod.__dict__)
print("plain hasattr:", hasattr(plain_mod, "__annotations__"))
print("plain after hasattr:", list(plain_mod.__dict__))
print("plain annotations value:", plain_mod.__annotations__)
print(
    "plain annotations identity:",
    plain_mod.__annotations__ is plain_mod.__dict__["__annotations__"],
)
del plain_mod.__annotations__
print("plain after del annotations:", list(plain_mod.__dict__))
ann_fresh = plain_mod.__annotations__
print("plain after re-access:", list(plain_mod.__dict__), ann_fresh)

# Imported annotated .py module: __annotations__ follows __builtins__ and
# precedes user bindings even when the first statement is unannotated.
import _globals_order_ann_2919 as ann_mod

print("ann keys:", list(ann_mod.__dict__))
print("ann annotations:", ann_mod.__annotations__)

# Imported module with del+rebind and global-only writes: both in-module
# globals() and external mod.__dict__ agree with CPython.
import _globals_order_ops_2919 as ops_mod

print("ops dunders:", [k for k in ops_mod.__dict__ if k.startswith("__")])
print("ops inner_keys:", ops_mod.inner_keys)
print("ops dict keys:", [k for k in ops_mod.__dict__ if not k.startswith("__")])
print("ops values:", ops_mod.a, ops_mod.b, ops_mod.c, ops_mod.g_only, ops_mod.d, ops_mod.e)

# An exec namespace is an ordinary dict: full insertion-order semantics,
# including del + re-assign moving the key to the end.
ns = {}
exec("one = 1\ntwo = 2\nthree = 3\ndel two\ntwo = 22\none = 11", ns)
print("exec ns:", [k for k in ns if not k.startswith("_")])
print("exec values:", ns["one"], ns["two"], ns["three"])

# locals() at module scope is the same namespace as globals().
print("locals == globals keys:", list(locals()) == list(globals()))
