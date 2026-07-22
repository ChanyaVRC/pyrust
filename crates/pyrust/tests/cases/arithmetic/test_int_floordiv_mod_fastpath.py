# int-int `//` and `%` VM fast path (int_int_fast).  Operands are read from a
# list at runtime so the values are NOT constant-folded by the optimizer —
# this forces the BinOp through the runtime int-int fast path, which must match
# CPython 3.12's floor-division / floor-modulo semantics exactly (sign of
# divisor, i64::MIN edge cases, zero-divisor errors, bool-as-int).

MIN64 = -9223372036854775808
MAX64 = 9223372036854775807

# (dividend, divisor) pairs spanning every sign combination plus the i64
# boundary cases.  Read from the list at runtime to defeat constant folding.
pairs = [
    (7, 2),
    (-7, 2),
    (7, -2),
    (-7, -2),
    (8, 2),
    (-8, 2),
    (8, -2),
    (-8, -2),
    (0, 5),
    (0, -5),
    (1, 7),
    (-1, 7),
    (MAX64, 3),
    (MIN64, 3),
    (MAX64, -3),
    (MIN64, -3),
    (MIN64, -1),  # floor-div overflows i64 -> BigInt (2**63); mod -> 0
    (MAX64, MAX64),
    (MIN64, MIN64),
]

for a, b in pairs:
    print(a, "//", b, "=", a // b)
    print(a, "%", b, "=", a % b)
    print(a, "divmod", b, "=", divmod(a, b))

# Augmented-assign forms (BinOpInPlace / BinOpConst path also tries int_int_fast).
acc = 1000
for _, b in pairs:
    acc //= b if b != 0 else 1
    acc %= 1000003
print("aug acc =", acc)

# Bool operands: `as_int()` accepts bool, result stays int (CPython parity).
bools = [True, False]
for a in bools:
    for b in [1, 2, 3]:
        print(repr(a), "//", b, "=", a // b)
        print(repr(a), "%", b, "=", a % b)
for b in bools:
    # `x % True` / `x // True` -> divisor is 1
    print("5 //", repr(b if b else 1), "=", 5 // (b if b else 1))
    print("5 %", repr(b if b else 1), "=", 5 % (b if b else 1))
print("True // True =", True // True)
print("True % True =", True % True)


# Zero-divisor must raise ZeroDivisionError with the right message.
def show_err(fn):
    try:
        print(fn())
    except Exception as e:
        print(type(e).__name__ + ": " + str(e))


z = [3, 0]  # runtime zero, not a literal
show_err(lambda: z[0] // z[1])
show_err(lambda: z[0] % z[1])
show_err(lambda: divmod(z[0], z[1]))
