# int-int `/` (true division) VM fast path (int_int_fast).  Operands are read
# from a list at runtime so they are NOT constant-folded — this forces the
# BinOp through the runtime int-int fast path.  `/` always yields a float and
# must match CPython 3.12 byte-for-byte, including the boundary between the
# inlined exactly-representable case (|n| < 2^53) and the exact big-integer
# divider used for larger magnitudes (#1923).

P53 = 1 << 53  # 9007199254740992

pairs = [
    (7, 2),
    (-7, 2),
    (7, -2),
    (-7, -2),
    (6, 3),
    (-6, 3),
    (1, 3),
    (2, 3),
    (0, 5),
    (0, -5),
    (10, 4),
    (-10, 4),
    (1, 8),
    (5, 1),
    (-5, 1),
    (100, 7),
    # exact-representable boundary (both < 2^53): stays on the inline fast path
    (P53 - 1, 3),
    (P53 - 1, P53 - 1),
    (-(P53 - 1), 7),
    # operands at / beyond 2^53: must fall through to the exact bigint divider
    (P53, 3),
    (P53 + 1, 2),
    (9223372036854775807, 3),  # i64::MAX
    (-9223372036854775808, 3),  # i64::MIN
    (9223372036854775807, 9223372036854775807),
    (10**30, 3),  # BigInt operand (bypasses as_int; slow path)
    (10**30, 10**15),
]

for a, b in pairs:
    print(a, "/", b, "=", repr(a / b))

# Augmented assign `x /= y` (BinOpInPlace also tries int_int_fast).
acc = 1_000_000
for _, b in pairs:
    acc /= b
print("aug acc =", repr(acc))

# Bool operands: `as_int()` accepts bool; `/` still yields float.
for a in [True, False]:
    for b in [1, 2, 3]:
        print(repr(a), "/", b, "=", repr(a / b))
print("7 / True =", repr(7 / True))
print("True / True =", repr(True / True))


# Zero divisor must raise ZeroDivisionError with the right message.
def show_err(fn):
    try:
        print(fn())
    except Exception as e:
        print(type(e).__name__ + ": " + str(e))


z = [3, 0]  # runtime zero, not a literal
show_err(lambda: z[0] / z[1])
