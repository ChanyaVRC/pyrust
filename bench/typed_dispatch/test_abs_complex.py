# Typed-dispatch microbench: `abs(complex)` (#399).
#
# `complex` doesn't match the first three `abs` overloads
# (`PyInt`, `PyFloat`, `PyBool`) and falls through to the trailing
# `PyValue` catch-all — three predicate misses before the match.  This is
# the *worst case* for the per-overload cascade on the currently migrated
# builtins; if it stays cheap, the cascade is acceptable.
N = 1_000_000
c = complex(3, 4)
for _ in range(N):
    abs(c)
