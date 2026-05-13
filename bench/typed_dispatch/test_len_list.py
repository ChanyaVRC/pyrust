# Typed-dispatch microbench: `len(list)` (#399).
#
# `len` is currently **legacy** `(args)`-form — manual kwarg rejection,
# arity check, then a `match` on `value.kind()`.  This is what we'd
# replace with a typed-overload definition; the bench number gates that
# migration decision.  `len(list)` is a very hot path — every `for x in
# list` doesn't strictly call it, but user-level `len(...)` calls are
# common in inner loops.
N = 1_000_000
xs = [0, 1, 2, 3, 4]
for _ in range(N):
    len(xs)
