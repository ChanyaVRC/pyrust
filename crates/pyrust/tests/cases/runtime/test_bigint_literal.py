# Integer literals larger than i64::MAX must parse as BigInt, not fail with a
# lex error.  Arithmetic promotion already works at runtime (PR #484); this
# fixture covers the *literal* path.

# Large decimal literal
x = 9999999999999999999999999999999
print(x)          # 9999999999999999999999999999999
print(type(x).__name__)  # int

# Large hex literal
y = 0xDEADBEEFDEADBEEFDEADBEEF
print(y)
print(y > 0)      # True

# Large octal literal (0o1000000000000000000000 == 2^63 == 9223372036854775808)
o = 0o1000000000000000000000
print(o == 9223372036854775808)  # True

# Large binary literal (2^63 in binary — 64 binary digits)
b = 0b1000000000000000000000000000000000000000000000000000000000000000
print(b == 9223372036854775808)  # True

# Underscore separators in large decimal literals
c = 100_000_000_000_000_000_000
print(c)          # 100000000000000000000

# Round-trip through arithmetic
a = 9999999999999999999999 + 1
print(a)          # 10000000000000000000000

# Small literals are still regular ints, not BigInt
small = 42
print(type(small).__name__)  # int
print(small)                 # 42

# i64::MAX is still a plain int, i64::MAX+1 promotes to BigInt
print(9223372036854775807)   # 9223372036854775807
print(9223372036854775808)   # 9223372036854775808

# Negation of a BigInt literal (fold_constant covers this via Unary::Neg)
neg = -9999999999999999999999999999999
print(neg)        # -9999999999999999999999999999999
