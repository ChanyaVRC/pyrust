# Loop-body LoadConst+BinOp fusion (perf/superinstructions).
#
# The const-fusion pass now fuses the "reused scratch register" shape inside
# loop bodies (a temp re-LoadConst-ed per operand, with the loop's back-edge
# following).  These cases lock in the *values* across the boundary inputs that
# the fusion + downstream strength-reduction must preserve byte-for-byte.

# Multiple consts per iteration over a reused scratch temp.
s = 0
for i in range(2000):
    s += i * 2 - 1
print(s)

# Chained arithmetic with several distinct constants (mixed bench shape).
acc = 0
for i in range(1500):
    a = i * 3 + 7
    b = a - i * 2
    acc += a + b - 1
print(acc)

# Negative step and a constant whose value sits at the i16 immediate boundary.
t = 0
for i in range(-5, 5):
    t += i * 32767 + 1
print(t)

# BigInt operands must NOT be folded by the `x + 0` / `x * 1` algebraic
# identities: those rewrite to a Move and would alias the object, but CPython
# allocates a fresh BigInt (object identity is observable via `is`).  Loosening
# the const fusion exposed this latent case (issue #523).
big = 2 ** 70
print((big + 0) is big)   # False: fresh object
print((big * 1) is big)   # False: fresh object
print((big - 0) is big)   # False: fresh object
print(big + 0 == big)     # True: equal value
print(big * 1 == big)     # True

# Value-correctness of the same identities on a BigInt inside a loop body.
total = 0
for _ in range(3):
    total += big + 0
print(total == big * 3)
print(total)
