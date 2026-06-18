# Regression for issue #2565: a ternary (conditional expression) used as the
# right operand of a binary op must not fold the else-branch constant into the
# whole op.  The two branches each emit their own LoadConst feeding a shared
# trailing BinOp; the binop-const fusion pass used to drop the else-branch load
# and rewrite the BinOp into BinOpConst, so the then-branch (which jumps onto
# the BinOp) wrongly used the else constant.


def f(a):
    return a + (10 if a else 20)


print(f(5))  # 15, not 25
print(f(0))  # 20

# Module scope reproduces the same shape.
a = 5
print(a + (10 if a else 20))  # 15
print((10 if a else 20) + a)  # ternary as left operand: 15

# Other binary ops with a ternary right operand.
print(a - (10 if a else 20))  # -5
print(a * (10 if a else 20))  # 50

# Standalone ternary (no surrounding binop) still correct.
print(10 if a else 20)  # 10
print(10 if not a else 20)  # 20

# Nested ternary as the else branch.
b = 0
print(a + (1 if b else (2 if a else 3)))  # 7


# Non-constant branches go through a different (non-fused) path.
def g(x, y, z):
    return x + (y if x else z)


print(g(5, 100, 200))  # 105
print(g(0, 100, 200))  # 200

# Both branches the same constant: fusion of unrelated pairs still works.
print(a + (9 if a else 9))  # 14

# Ternary chained between two binops.
c = 3
print(a + (10 if a else 20) + c)  # 18

# Ternary feeding a comparison.
print(a == (5 if a else 6))  # True

# Ternary as the operand of a unary op (same jump-target hazard in the unary
# fold pass): the then- and else-branch each emit their own LoadConst feeding a
# shared trailing UnaryOp, which the then-branch jumps onto.
print(-(10 if a else 20))  # -10, not -20
print(-(10 if not a else 20))  # -20
print(~(10 if a else 20))  # -11
print(not (10 if a else 0))  # False
print(not (10 if not a else 0))  # True
