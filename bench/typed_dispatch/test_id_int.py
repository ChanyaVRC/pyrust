# Legacy-form baseline for the typed-dispatch microbench (#399).
#
# `id` is a one-arg legacy `(args)`-form builtin: manual kwarg-reject,
# arity check, kind-match on the argument.  It does *less* work than
# `abs(int)` (no `as_i64`/`checked_abs`/BigInt fallback) but its
# dispatch shape — one positional, no kwargs — is the closest legacy
# analogue to `abs(int)`.  The ratio between this and `abs(int)` is the
# overload-dispatch overhead for the first-overload-matches case.
N = 1_000_000
x = -1234567
for _ in range(N):
    id(x)
