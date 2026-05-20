# Parity fixture: pass_reassoc — compile-time reassociation of (x op c1) op c2.
# The pass rewrites (x op c1) op c2 → x op (c1 op c2) so that pass_const_fold
# can fold the two constants together.

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

# ── Float: must NOT be reassociated (floating-point non-associativity) ────────

def float_no_reassoc(a):
    # (a + 1e100) + (-1e100) == 0.0 under CPython's left-to-right eval.
    # Reassociating to a + (1e100 + -1e100) = a + 0.0 = a would change the result.
    return (a + 1e100) + (-1e100)

assert float_no_reassoc(1.0) == 0.0, float_no_reassoc(1.0)

# ── String: must NOT be reassociated ─────────────────────────────────────────

def str_concat(s):
    return (s + " ") + "world"

assert str_concat("hello") == "hello world", str_concat("hello")

# ── Overflow: must NOT reassociate when constant fold overflows i64 ───────────

# i64 max = 9223372036854775807
# (max - 1) + 1 = max; max + 1 overflows → BigInt.  The reassociation of
# (x + (max-1)) + 1 → x + max is fine, but (x + max) + 1 → x + (max+1)
# would overflow at compile time; we must bail and get runtime BigInt semantics.
i64_max = 9223372036854775807

def overflow_add(x):
    return (x + i64_max) + 1

# With x=0 the result is i64_max + 1 = 9223372036854775808 (BigInt).
result = overflow_add(0)
assert result == 9223372036854775808, result

print("reassoc OK")
