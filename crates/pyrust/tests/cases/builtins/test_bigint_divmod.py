# Parity fixture for floor-division sign handling in divmod/floorediv/modulo.
# Issue #1461: divmod(-2**63, 3) returned wrong quotient sign because the
# i64 fast path computed `(a - modulo) / b` which overflows near i64 boundaries.
# Also: divmod(-2**63, -1) panicked due to `i64::MIN % -1` overflow in py_mod_i64.

# ── All four sign combinations for small ints ─────────────────────────────────

print(divmod(-7, 3))    # (-3, 2)
print(divmod(7, -3))    # (-3, -2)
print(divmod(-7, -3))   # (2, -1)
print(divmod(7, 3))     # (2, 1)

# ── i64 boundary cases — previously wrong quotient sign ──────────────────────

# a=i64::MIN, b=3: quotient fits i64 but (a - modulo) underflows in the fast path
print(divmod(-2**63, 3))    # (-3074457345618258603, 1)

# a=i64::MAX, b=-2: quotient fits i64 but (a - modulo) overflows in the fast path
print(divmod(2**63 - 1, -2))  # (-4611686018427387904, -1)

# a=i64::MIN, b=-1: quotient = 2^63, which doesn't fit in i64 — must use BigInt
print(divmod(-2**63, -1))   # (9223372036854775808, 0)

# a=i64::MIN, b=1: quotient = i64::MIN (fits)
print(divmod(-2**63, 1))    # (-9223372036854775808, 0)

# a=i64::MIN, b=-3
print(divmod(-2**63, -3))   # (3074457345618258602, -2)

# ── // and % operators with the same boundary values ─────────────────────────

print((-2**63) // 3)    # -3074457345618258603
print((-2**63) % 3)     # 1
print((-2**63) // -1)   # 9223372036854775808
print((-2**63) % -1)    # 0
print((2**63 - 1) // -2)  # -4611686018427387904
print((2**63 - 1) % -2)   # -1

# ── Large BigInt cases (> i64::MAX, always go through BigInt path) ────────────

print(divmod(2**100, 7))    # (181092942889747057356671886482, 2)
print(divmod(-2**100, 7))   # (-181092942889747057356671886483, 5)
print(divmod(2**100, -7))   # (-181092942889747057356671886483, -5)
print(divmod(-2**100, -7))  # (181092942889747057356671886482, -2)

# ── Invariant: b*q + r == a, and 0 <= r < abs(b) (or b < r <= 0 for b < 0) ──

for a, b in [(-2**63, 3), (-2**63, -3), (-2**63, -1), (2**63 - 1, -2),
             (2**100, 7), (-2**100, -7)]:
    q, r = divmod(a, b)
    assert b * q + r == a, f"invariant failed: divmod({a}, {b}) = ({q}, {r})"
    if b > 0:
        assert 0 <= r < b, f"remainder out of range: divmod({a}, {b}) = ({q}, {r})"
    else:
        assert b < r <= 0, f"remainder out of range: divmod({a}, {b}) = ({q}, {r})"

print("all invariants ok")

# ── ZeroDivision still raises ─────────────────────────────────────────────────

try:
    divmod(-2**63, 0)
except ZeroDivisionError as e:
    print("ZeroDivisionError:", e)

try:
    (-2**63) // 0
except ZeroDivisionError as e:
    print("ZeroDivisionError:", e)

try:
    (-2**63) % 0
except ZeroDivisionError as e:
    print("ZeroDivisionError:", e)
