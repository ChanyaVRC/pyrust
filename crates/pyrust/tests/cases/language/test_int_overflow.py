# Issue #421: integer overflow must promote to BigInt instead of wrapping.
#
# Python ints are arbitrary precision; every `+ - * **` path in pyrust must
# either stay inside `i64` or promote to BigInt — never silently wrap.  This
# fixture exercises the boundary at i64::MAX (≈9.22e18) so a regression that
# reintroduces `wrapping_*` would flip a sign in the output.

# ---- compile-time constant fold (`pass_const_fold` / `fold_binop`) ----
print(9223372036854775000 + 1000)
print(9223372036854775000 - -1000)
print(4611686018427387904 * 2)
print(2 ** 64)
print(2 ** 100)
print(10 ** 20)

# ---- runtime fall-through (`expr::add/sub/mul/Pow`) ----
def add_runtime():
    a = 9223372036854775000
    b = 1000
    return a + b
print(add_runtime())

def mul_runtime():
    a = 4611686018427387904
    return a * 2
print(mul_runtime())

def pow_runtime():
    a = 2
    b = 64
    return a ** b
print(pow_runtime())

# ---- AST-walk spec cache (`helpers::eval_binary_int`) ----
# Repeatedly run the same op so the spec cache promotes to a specialised path,
# then trigger overflow on the specialised path.
def spec_warmup():
    total = 0
    for _ in range(20):
        total = 2 ** 30
    return total
print(spec_warmup())

# After the warmup, a much larger exponent on the same op must still promote.
def spec_overflow():
    out = 0
    for _ in range(2):
        out = 2 ** 70
    return out
print(spec_overflow())

# ---- pow() builtin ----
print(pow(2, 64))
print(pow(10, 20))
print(pow(True, 64))

# ---- non-overflowing fast path still works ----
print(2 ** 62)
print(4611686018427387904 - 1)
print(1000 * 1000 * 1000)

# ---- negative exponent stays float ----
print(2 ** -1)
print(pow(2, -1))

# ---- float ** stays float ----
print(2.0 ** 3)
print(2 ** 3.0)

# ---- negative base, integer exponent ----
print((-3) ** 4)
print((-3) ** 3)
print((-2) ** 63)

# ---- arithmetic across the i64 boundary keeps working ----
# After an op promotes to BigInt, subsequent ops with Int / BigInt operands
# must continue to compute (not raise TypeError).  Repro for the followup
# observed while writing the fix.
big = 2 ** 64
print(big + 1)
print(1 + big)
print(big - 1)
print(1 - big)
print(big * 2)
print(2 * big)
print(big - big)
print(big + big)
print(big * big)

# Unary minus on a BigInt
print(-big)
print(-(2 ** 64))
