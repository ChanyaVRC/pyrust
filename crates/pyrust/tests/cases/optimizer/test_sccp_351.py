# Issue #351: Sparse Conditional Constant Propagation (SCCP) end-to-end parity
# suite against CPython 3.12.3.

import math


# 1. Dead branch propagation + cascading condition elimination
def dead_branch_cascade():
    x = 1
    if False:
        x = 5
    if x == 5:
        return 100
    return x + 10


print("1_cascade", dead_branch_cascade())


# 2. Constant if/else folding + downstream arithmetic
def const_if_else():
    if True:
        x = 2
    else:
        x = 3
    y = -x
    return (x + 5, y * 3)


print("2_if_else", const_if_else())


# 3. Diamond merge where both reachable branches agree on the same constant
def diamond_agree(flag):
    if flag:
        x = 7
    else:
        x = 7
    if x == 7:
        return x * 6
    return -1


print("3_diamond_agree", diamond_agree(True), diamond_agree(False))


# 4. Diamond merge where reachable branches disagree -> must NOT fold
def diamond_disagree(flag):
    if flag:
        x = 7
    else:
        x = 8
    return x + 1


print("4_diamond_disagree", diamond_disagree(True), diamond_disagree(False))


# 5. Loop induction variable must converge to Bottom (never fold)
def loop_induction():
    i = 0
    total = 0
    while i < 10:
        total += i
        i += 1
    return (i, total)


print("5_loop_induction", loop_induction())


# 6. Loop with provably false entry condition (unreachable back-edge)
def dead_loop():
    i = 0
    while i > 10:
        i += 1
    return i + 5


print("6_dead_loop", dead_loop())


# 7. Nested dead branches with None identity checks
def none_identity():
    x = None
    if x is not None:
        return "bad"
    if x is None:
        return "good"
    return "unreachable"


print("7_none_identity", none_identity())


# 8. Exception handler soundness: must not fold across raise / try-except
def exc_soundness_raise():
    x = 0
    try:
        x = 1
        raise ValueError("boom")
        x = 2
    except ValueError:
        return x + 10


def exc_soundness_divzero():
    x = 0
    try:
        x = 3
        y = 1 // 0
        x = 4
        return y
    except ZeroDivisionError:
        return x * 10


def exc_soundness_try_in_dead_branch():
    x = 9
    if False:
        try:
            x = 99
            raise RuntimeError("never")
        except RuntimeError:
            x = 88
    return x + 1


print("8_exc", exc_soundness_raise(), exc_soundness_divzero(), exc_soundness_try_in_dead_branch())


# 9. UnboundLocalError preservation when only one branch assigns
def maybe_unbound(flag):
    if flag:
        x = 1
    return x + 1


try:
    maybe_unbound(False)
    print("9_unbound", "NO_ERR")
except UnboundLocalError:
    print("9_unbound", "UnboundLocalError", maybe_unbound(True))


# 10. del local invalidation -> UnboundLocalError
def del_local_after_const():
    x = 42
    del x
    return x


try:
    del_local_after_const()
    print("10_del", "NO_ERR")
except UnboundLocalError:
    print("10_del", "UnboundLocalError")


# 11. Type preservation: Int(1) vs Bool(True) must never unify
def int_vs_bool(flag):
    if flag:
        x = 1
    else:
        x = True
    return (x, type(x).__name__)


print("11_int_bool", int_vs_bool(True), int_vs_bool(False))


# 12. Signed zero preservation: +0.0 vs -0.0 must never unify
def signed_zero(flag):
    if flag:
        x = 0.0
    else:
        x = -0.0
    return math.copysign(1.0, x)


print("12_signed_zero", signed_zero(True), signed_zero(False))


# 13. NaN comparisons & truthiness
def nan_const():
    nan = float("nan")
    return (bool(nan), nan == nan, nan != nan, nan < 0.0, nan > 0.0)


print("13_nan", nan_const())


# 14. Division by zero and invalid unary op must raise at runtime, never fold
def div_zero_runtime():
    x = 1
    return x / 0


try:
    div_zero_runtime()
    print("14_divzero", "NO_ERR")
except ZeroDivisionError:
    print("14_divzero", "ZeroDivisionError")


def bad_unary_runtime():
    x = 1.5
    return ~x


try:
    bad_unary_runtime()
    print("14_unary", "NO_ERR")
except TypeError:
    print("14_unary", "TypeError")


# 15. BigInt overflow promotion across constant branches
def bigint_const():
    if True:
        x = 1 << 70
    else:
        x = 0
    return x + 1


print("15_bigint", bigint_const())


# 16. Closure cell variable mutated across call must not be folded
def closure_mutation():
    x = 1
    def bump():
        nonlocal x
        x = 2
    bump()
    return x + 10


print("16_closure", closure_mutation())


# 17. Generator yield with live locals
def gen_sccp(flag):
    x = 1
    if False:
        x = 99
    yield x + 1
    if flag:
        x = 2
    yield x + 1


print("17_gen", list(gen_sccp(True)), list(gen_sccp(False)))


# 18. String constant propagation across branches
def str_const():
    a = "foo"
    if False:
        a = "nope"
    b = a + "bar"
    if b == "foobar":
        return b + "!"
    return "wrong"


print("18_str", str_const())
