# Typed-dispatch microbench: `abs(int)` (#399).
#
# `abs` is the first migrated overload-dispatched builtin (#395).  The
# `PyInt` overload is declared first in `builtin_modules/bodies/builtins.rs`,
# so an int argument matches on the first `matches`-predicate probe — this
# is the *best case* for the per-overload cascade.  Compare ns/call against
# `test_id_int.py` (legacy `(args)` form, similarly trivial body) to size
# the overload-dispatch overhead.
N = 1_000_000
x = -1234567
for _ in range(N):
    abs(x)
