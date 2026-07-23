# Unary -/+/~ VM fast path on tagged ints (and bool).  Operands are read from a
# list at runtime so they are NOT constant-folded — this forces the UnaryOp
# through the runtime int fast path, which must match CPython 3.12 exactly,
# including bool operands (result is int) and the `-i64::MIN` overflow that
# promotes to BigInt.

MIN64 = -9223372036854775808
MAX64 = 9223372036854775807

vals = [0, 1, -1, 2, -2, 7, -7, 100, -100, MAX64, MIN64, MAX64 - 1, MIN64 + 1]

for v in vals:
    print(v, "-> neg", -v, "pos", +v, "inv", ~v)

# -i64::MIN must promote to BigInt (2**63), not wrap.
big = [MIN64, -(MIN64)]  # second is already BigInt 2**63
for v in big:
    print("neg", v, "=", -v, "inv", v, "=", ~v)

# Bool operands: as_int() accepts bool; results are int (CPython parity).
# (`~bool` is intentionally omitted — CPython 3.12 emits a DeprecationWarning
# for it, which is version-specific stderr noise; `~int(b)` is covered above.)
for b in [True, False]:
    print(repr(b), "-> neg", -b, "pos", +b)

# Double / nested unary.
for v in [5, -5, 0]:
    print(v, "-> --", - -v, "~~", ~~v, "-~", -~v, "~-", ~-v)

# Unary on BigInt operands (bypass as_int; slow path) stays correct.
B = 10**25
print("neg bigint =", -B, "inv bigint =", ~B, "pos bigint =", +B)

# `not` is unchanged (dispatches truthy); spot-check it still works on ints.
for v in [0, 1, -1, 5]:
    print("not", v, "=", not v)
