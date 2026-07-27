# Parity fixture for left-associated arithmetic and protocol dispatch.
# The optimizer must preserve source evaluation order for dynamic operands;
# fully constant expressions are handled by ordinary forward constant folding.

# ── Integer Add ───────────────────────────────────────────────────────────────

def add_chain(a):
    return (a + 1) + 2

assert add_chain(5) == 8, add_chain(5)
assert add_chain(0) == 3, add_chain(0)
assert add_chain(-10) == -7, add_chain(-10)

# ── Integer Mul ───────────────────────────────────────────────────────────────

def mul_chain(x):
    return (x * 2) * 5

assert mul_chain(3) == 30, mul_chain(3)
assert mul_chain(0) == 0, mul_chain(0)
assert mul_chain(-1) == -10, mul_chain(-1)

# ── Bitwise Or ────────────────────────────────────────────────────────────────

def bitor_chain(x):
    return (x | 0b0001) | 0b0100

assert bitor_chain(0b1010) == 0b1111, bin(bitor_chain(0b1010))

# ── Bitwise And ───────────────────────────────────────────────────────────────

def bitand_chain(x):
    return (x & 0b1111) & 0b1010

assert bitand_chain(0b1010) == 0b1010, bin(bitand_chain(0b1010))

# ── Bitwise XOR ───────────────────────────────────────────────────────────────

def bitxor_chain(x):
    return (x ^ 0b1010) ^ 0b0001

assert bitxor_chain(0b1100) == (0b1100 ^ 0b1010 ^ 0b0001), bin(bitxor_chain(0b1100))

# ── Three-step chain: a + b + 1 + 2 ──────────────────────────────────────────

def triple_add(a, b):
    return a + b + 1 + 2

assert triple_add(2, 3) == 8, triple_add(2, 3)

# ── Float: preserve left-to-right evaluation ─────────────────────────────────

def float_no_reassoc(a):
    # (a + 1e100) + (-1e100) == 0.0 under CPython's left-to-right eval.
# Regrouping to a + (1e100 + -1e100) = a + 0.0 = a would change the result.
    return (a + 1e100) + (-1e100)

assert float_no_reassoc(1.0) == 0.0, float_no_reassoc(1.0)

# ── String: preserve the two distinct additions ──────────────────────────────

def str_concat(s):
    return (s + " ") + "world"

assert str_concat("hello") == "hello world", str_concat("hello")

# ── Overflow: must NOT reassociate when constant fold overflows i64 ───────────

# i64 max = 9223372036854775807
# (max - 1) + 1 = max; max + 1 overflows → BigInt. Evaluation must retain
# runtime arbitrary-precision semantics rather than precomputing an i64 result.
i64_max = 9223372036854775807

def overflow_add(x):
    return (x + i64_max) + 1

# With x=0 the result is i64_max + 1 = 9223372036854775808 (BigInt).
result = overflow_add(0)
assert result == 9223372036854775808, result

# ── User-defined __add__: must produce correct result ─────────────────────────
#
# class C whose __add__ returns other * 10 (NOT self + other).
# (obj + 1) + 2:
#   step 1: obj.__add__(1)  → 10   (1 * 10)
#   step 2: (10).__add__(2) → 12   (plain int add)
# Regrouping would instead call obj.__add__(3) → 30, which is wrong.

class MultiplierAdd:
    def __add__(self, other):
        return other * 10

def user_add_chain(obj):
    return (obj + 1) + 2

result = user_add_chain(MultiplierAdd())
assert result == 12, f"expected 12, got {result}"

print("left-associative ops OK")
