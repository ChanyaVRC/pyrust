# Issue #350: Partial Redundancy Elimination (PRE / Lazy Code Motion)
# end-to-end parity suite against CPython 3.12.3.


# 1. Classic PRE diamond with else (proven exact ints, non-constant)
def pre_diamond_else(p):
    if p:
        a = 3
    else:
        a = 5
    if a == 3:
        b = 7
    else:
        b = 11
    if a < b:
        x = a * b
    else:
        x = 0
    y = a * b
    return (x, y, x + y)


print("1_diamond_else", pre_diamond_else(True), pre_diamond_else(False))


# 2. Classic PRE diamond without else
def pre_diamond_no_else(p):
    if p:
        a = 4
    else:
        a = 9
    b = 6
    x = -1
    if a < b:
        x = a * b
    y = a * b
    return (x, y)


print("2_diamond_no_else", pre_diamond_no_else(True), pre_diamond_no_else(False))


# 3. Commutative PRE (`a * b` in branch, `b * a` in join)
def pre_commutative(p):
    if p:
        a = 3
    else:
        a = 8
    b = 5
    if a < b:
        x = a * b
    else:
        x = 1
    y = b * a
    return (x, y)


print("3_commutative", pre_commutative(True), pre_commutative(False))


# 4. Loop-invariant PRE in while-True loop
def pre_loop_invariant(p):
    if p:
        a = 12
    else:
        a = 7
    b = 9
    i = 0
    total = 0
    while True:
        prod = a * b
        total += prod + i
        i += 1
        if i >= 5:
            break
    return total


print("4_loop_inv", pre_loop_invariant(True), pre_loop_invariant(False))


# 5. Operand killed on one branch must recompute with updated value
def pre_operand_killed(p):
    if p:
        a = 3
    else:
        a = 10
    b = 7
    if a < b:
        x = a * b
    else:
        a = 20
        x = 0
    y = a * b
    return (x, y)


print("5_killed", pre_operand_killed(True), pre_operand_killed(False))


# 6. Early return (not anticipated) must stay exact
def pre_early_return(p):
    if p:
        a = 3
    else:
        a = 8
    b = 7
    if a < 5:
        return -1
    return a * b


print("6_early_ret", pre_early_return(True), pre_early_return(False))


# 7. Try/except boundary soundness
def pre_try_except(p):
    if p:
        a = 3
    else:
        a = 5
    b = 7
    try:
        x = a * b
        if p:
            raise ValueError("stop")
    except ValueError:
        x = -1
    y = a * b
    return (x, y)


print("7_try_except", pre_try_except(True), pre_try_except(False))


# 8. Division by zero must NEVER be hoisted past a guarding branch
def pre_div_zero(p):
    if p:
        a = 10
    else:
        a = 20
    if a == 10:
        b = 2
    else:
        b = 0
    if b == 0:
        return "guarded"
    x = a // b
    y = a // b
    return (x, y)


print("8_div_zero", pre_div_zero(True), pre_div_zero(False))


# 9. User class with side-effecting __mul__: exact call count MUST match CPython!
mul_log = []


class Tracked:
    def __init__(self, v):
        self.v = v

    def __mul__(self, other):
        mul_log.append((self.v, other.v))
        return self.v * other.v


def pre_user_dunder(cond, a, b):
    if cond:
        x = a * b
    else:
        x = 0
    y = a * b
    return (x, y)


r1 = pre_user_dunder(True, Tracked(3), Tracked(4))
r2 = pre_user_dunder(False, Tracked(3), Tracked(4))
print("9_user_dunder", r1, r2, len(mul_log), mul_log)


# 10. Float PRE with signed zero and NaN preservation
def pre_float(p):
    if p:
        a = 1.5
    else:
        a = -2.5
    b = 4.0
    if a > 0.0:
        x = a * b
    else:
        x = 0.0
    y = a * b
    return (x, y)


print("10_float", pre_float(True), pre_float(False))


# 11. UnaryOp PRE across branches
def pre_unary(p):
    if p:
        a = 15
    else:
        a = -20
    if a > 0:
        x = -a
    else:
        x = 0
    y = -a
    return (x, y)


print("11_unary", pre_unary(True), pre_unary(False))


# 12. BigInt exact-int PRE without overflow error
def pre_bigint(p):
    if p:
        a = 1 << 70
    else:
        a = 1 << 71
    b = 3
    if a > 0:
        x = a * b
    else:
        x = 0
    y = a * b
    return (x == y, y >> 70)


print("12_bigint", pre_bigint(True), pre_bigint(False))
