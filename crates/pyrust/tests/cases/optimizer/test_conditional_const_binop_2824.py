# Issue #2824: conditional-const binop regression fixture.
#
# When a register `k` is assigned a constant (`0` or `1`) in one branch and a
# dynamic value in a sibling branch (or mutated across loop iterations), the
# optimizer must NOT associate the sibling branch's constant with a `BinOp`
# using `k` — neither inside the sibling `elif`/`else` branch, nor after the
# `if`/`else` merge, nor across loop back-edges.

from decimal import Decimal


# ── 1. Exact issue #2824 repro & Pow variants ────────────────────────────────
def pow_after_merge_zero(f, cond):
    if cond:
        k = 0
    else:
        k = f
    return (k, 5**k)


def pow_after_merge_one(f, cond):
    if cond:
        k = 1
    else:
        k = f
    return (k, 5**k)


def pow_in_sibling_elif(f):
    # Matches CPython's Decimal.from_float structure: `k = 0` in the `if`
    # branch and `k = dynamic; 5**k` inside the sibling `elif` branch.
    if isinstance(f, int):
        k = 0
        out = f
    elif isinstance(f, float):
        n, d = abs(f).as_integer_ratio()
        k = d.bit_length() - 1
        out = n * 5**k
    else:
        raise TypeError("expected int or float")
    return (k, out)


def pow_in_loop(xs):
    results = []
    for i, x in enumerate(xs):
        if i == 0:
            k = 0
        elif i == 1:
            k = 1
        else:
            k = x
        results.append((k, 5**k))
    return results


def pow_loop_carry(n):
    k = 0
    out = []
    i = 0
    while i < n:
        out.append(5**k)
        k = i + 1
        i += 1
    return out


def pow_ternary(f, cond):
    k = 0 if cond else f
    k1 = 1 if cond else f
    return (5**k, 5**k1)


print("pow_after_merge_zero:", pow_after_merge_zero(2, True), pow_after_merge_zero(2, False))
print("pow_after_merge_one:", pow_after_merge_one(3, True), pow_after_merge_one(3, False))
print("pow_in_sibling_elif:", pow_in_sibling_elif(7), pow_in_sibling_elif(0.25), pow_in_sibling_elif(0.125))
print("pow_in_loop:", pow_in_loop([9, 9, 2, 3, 0, 4]))
print("pow_loop_carry:", pow_loop_carry(5))
print("pow_ternary:", pow_ternary(3, True), pow_ternary(3, False))


# ── 2. Sibling ops (Add 0, Sub 0, Mul 1, Mul 0, and reversed operands) ───────
def sibling_ops_after_merge(base, dyn, cond):
    if cond:
        k0 = 0
        k1 = 1
    else:
        k0 = dyn
        k1 = dyn
    return (
        base + k0,
        k0 + base,
        base - k0,
        k0 - base,
        base * k1,
        k1 * base,
        base * k0,
        k0 * base,
    )


def sibling_ops_in_elif(base, dyn, cond):
    if cond:
        k0 = 0
        k1 = 1
        res = (base, k0, k1)
    elif not cond:
        k0 = dyn
        k1 = dyn + 1
        res = (
            base + k0,
            k0 + base,
            base - k0,
            k0 - base,
            base * k1,
            k1 * base,
            base * k0,
            k0 * base,
        )
    else:
        res = ()
    return res


def sibling_ops_in_loop(base, seq):
    out = []
    for idx, val in enumerate(seq):
        if idx % 3 == 0:
            k = 0
        elif idx % 3 == 1:
            k = 1
        else:
            k = val
        out.append((base + k, k + base, base - k, base * k, k * base, base**k))
    return out


print(
    "sibling_ops_after_merge:",
    sibling_ops_after_merge(7, 4, True),
    sibling_ops_after_merge(7, 4, False),
)
print(
    "sibling_ops_in_elif:",
    sibling_ops_in_elif(7, 4, True),
    sibling_ops_in_elif(7, 4, False),
)
print("sibling_ops_in_loop:", sibling_ops_in_loop(3, [10, 20, 2, 30, 40, 3]))


# ── 3. Genuine constant exponents still evaluate cleanly ─────────────────────
def genuine_const_pow():
    return (5**0, 5**1, 5**2, 2**0, 2**1, 2**10, 0**0, 1**100)


print("genuine_const_pow:", genuine_const_pow())


# ── 4. Decimal.from_float (original #2824 trigger) ───────────────────────────
for val in [0.25, 0.5, 0.1, -0.25, 0, -0.0, 42, -42, 0.125, 1.5, float("inf"), float("-inf")]:
    print(f"Decimal.from_float({val!r}):", repr(Decimal.from_float(val)))
print("Decimal.from_float(nan):", Decimal.from_float(float("nan")).is_nan())
