# Regression for issue #2002: a long single-variable def-use chain used to
# compile in O(n^2) time (const-folding interned each folded constant with a
# linear pool scan; remap_linenos / dead_store_elim / binop_const_fusion each
# re-scanned the whole instruction stream per statement).  The fix makes those
# passes linear; this fixture pins the *result* of folding a long chain so the
# optimizer can never silently change the value it produces.

# Foldable accumulator: every step is a constant after propagation.
x = 0
x = x + 1
x = x + 2
x = x + 3
x = x + 4
x = x + 5
x = x + 6
x = x + 7
x = x + 8
x = x + 9
x = x + 10
print(x)

# Mixed operators on one variable (exercises binop_const_fusion + const_fold).
y = 1
y = y * 2
y = y + 3
y = y * 4
y = y - 5
y = y * 6
print(y)

# A non-foldable seed keeps the chain partly dynamic: the dead-store /
# back-edge guards must still preserve every intermediate that is observed.
def seed():
    return 3


z = seed()  # opaque to the optimizer (a call result)
z = z + 10
z = z * 2
z = z - 1
print(z)

# Chain inside a loop: the back-edge guard must NOT let a folded store be
# dropped when the value is carried across iterations.
acc = 0
for i in range(100):
    acc = acc + i
print(acc)

# Long chain whose final value overflows i64 into BigInt (exercises the
# BigInt const-key path in the interner).
big = 1
for _ in range(70):
    big = big * 2
print(big)
