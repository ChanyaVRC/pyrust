# Mixed int/float BinOp fast path (BinopTypeTag::NumMixed).  Operands come from
# runtime lists so they are NOT constant-folded — forcing the inline cache to
# specialize on the mixed tag and take the coercion fast path.  Every result
# must match CPython 3.12, which likewise coerces the int operand to float.

# Warm each site >8 iterations (BINOP_SPEC_THRESHOLD) so it specializes, then
# check the values.
# Non-zero operands only: `run` divides in both directions, so a 0 operand
# would raise (that case is covered in the caught show_err block below, where
# the traceback isn't compared).
ints = [1, -1, 2, -2, 3, 7, -7, 10, -10, 100, 1000000007]
flts = [2.5, -2.5, 0.5, -0.5, 1.0, 3.0, 0.1, -0.1]


def run(op_name, fn):
    for a in ints:
        for b in flts:
            # int OP float
            print(op_name, a, b, "=", repr(fn(a, b)))
            # float OP int (reversed operand order)
            print(op_name, b, a, "=", repr(fn(b, a)))


run("add", lambda x, y: x + y)
run("sub", lambda x, y: x - y)
run("mul", lambda x, y: x * y)
run("div", lambda x, y: x / y)
run("floordiv", lambda x, y: x // y)
run("mod", lambda x, y: x % y)

# 0 operand in the non-dividing arithmetic ops (mixed fast path).
for b in flts:
    print("zero", 0, b, "=", repr(0 + b), repr(0 - b), repr(0 * b), repr(b + 0), repr(b * 0))

# Comparisons must stay EXACT (fall through, not coercion) — large int vs float.
BIG = 2**53 + 1
print("cmp", BIG, float(BIG), BIG == float(BIG), BIG < float(BIG), BIG > float(BIG))
for a in [2, 3, 7]:
    for b in [2.0, 2.5, 3.0]:
        print("cmp", a, b, a == b, a < b, a <= b, a > b, a >= b, a != b)

# Pow with mixed operands (must fall through; can yield complex for neg base).
print("pow", 2, 0.5, repr(2**0.5))
print("pow", 9, 0.5, repr(9**0.5))
print("pow", 2.0, 3, repr(2.0**3))

# Bool operands participate as int (1/0).
for b in [True, False]:
    print("bool", b, 2.5, repr(b + 2.5), repr(b * 2.5), repr(2.5 - b))

# Aug-assign mixed (BinOpInPlace also routes through the numeric path).
acc = 0.0
for i in ints:
    acc += i * 0.5
print("aug acc =", repr(acc))


# Div-by-zero with a float 0.0 divisor must raise (fall through).
def show_err(fn):
    try:
        print(fn())
    except Exception as e:
        print(type(e).__name__ + ": " + str(e))


z = [3, 0.0]
show_err(lambda: z[0] / z[1])
show_err(lambda: z[0] // z[1])
show_err(lambda: z[0] % z[1])
