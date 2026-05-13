# Typed-dispatch microbench: `getattr(obj, "attr")` (#399).
#
# `getattr` is currently **legacy** `(args)`-form, two required args plus
# an optional third (default).  A typed-signature migration would map to
# `(obj: PyValue, name: PyStr, default: PyValue = …)` — single-body, not
# overloaded.  Two-arg builtins are the more common hot-path shape; this
# bench numbers the migration cost.
class Obj:
    pass

o = Obj()
o.x = 42

N = 1_000_000
for _ in range(N):
    getattr(o, "x")
