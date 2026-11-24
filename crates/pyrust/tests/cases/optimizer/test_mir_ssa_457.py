# Issue #457: SSA-based MIR pipeline and core optimizer passes end-to-end
# parity test suite against CPython 3.12.3.

import sys


# 1. Phi parallel-copy de-SSA: 2-way and 3-way swap cycles in loops
def fib_swap(n):
    a = 0
    b = 1
    i = 0
    while i < n:
        a, b = b, a + b
        i += 1
    return a


def rotate_three(n):
    a, b, c = 1, 2, 3
    for _ in range(n):
        a, b, c = b, c, a
    return (a, b, c)


print("1_fib", [fib_swap(k) for k in range(10)])
print("1_rot", [rotate_three(k) for k in range(5)])


# 2. SCCP across diamonds, dead arms, and loops
def sccp_diamond(flag):
    x = 10
    if False:
        x = 999
    if flag:
        y = x + 5
    else:
        y = 15
    if y == 15:
        return y * 2
    return -1


def sccp_disagree(flag):
    if flag:
        v = 3
    else:
        v = 7
    return v * 4


print("2_sccp", sccp_diamond(True), sccp_diamond(False), sccp_disagree(True), sccp_disagree(False))


# 3. #438 type-guarded algebraic simplification vs custom dunders and bool
class TrackedOps:
    def __init__(self, label):
        self.label = label
        self.log = []

    def __add__(self, other):
        self.log.append(("add", other))
        return f"{self.label}+{other}"

    def __sub__(self, other):
        self.log.append(("sub", other))
        return f"{self.label}-{other}"

    def __mul__(self, other):
        self.log.append(("mul", other))
        return f"{self.label}*{other}"

    def __or__(self, other):
        self.log.append(("or", other))
        return f"{self.label}|{other}"

    def __and__(self, other):
        self.log.append(("and", other))
        return f"{self.label}&{other}"

    def __xor__(self, other):
        self.log.append(("xor", other))
        return f"{self.label}^{other}"


def run_algebraic_dynamic(x):
    return (x + 0, x - 0, x * 1, x * 0, x | 0, x & 0, x ^ 0)


t = TrackedOps("T")
print("3_dunder", run_algebraic_dynamic(t), t.log)


def run_algebraic_bool(flag):
    b = True if flag else False
    r_add = b + 0
    r_mul = b * 1
    r_sub = b - 0
    return (r_add, type(r_add).__name__, r_mul, type(r_mul).__name__, r_sub, type(r_sub).__name__)


print("3_bool", run_algebraic_bool(True), run_algebraic_bool(False))


# 4. #421 i64 overflow promotion to BigInt in SSA constant folding
def overflow_const_fold():
    a = (1 << 62) + (1 << 62)
    b = -(1 << 63)
    c = -b
    d = (1 << 62) * 4 + 7
    return (a, b, c, d)


print("4_overflow", overflow_const_fold())


# 5. #357 proven-Int constant reassociation vs unknown objects & floats
def reassoc_proven_int(n):
    acc = 0
    for i in range(n):
        # `i` and `acc` are proven Int in SSA MIR.
        x = (acc + 1) + 2
        y = (x - 5) + 3
        z = (y * 2) * 3
        w = ((z & 255) & 63) | 1 | 4
        acc = (w ^ 3) ^ 5
    return acc


class CustomAdd:
    def __add__(self, other):
        return ("custom", other)


def reassoc_unknown(x):
    return (x + 1) + 2


def reassoc_float(a):
    return (a + 1e100) + (-1e100)


print("5_reassoc_int", reassoc_proven_int(10))
try:
    reassoc_unknown(CustomAdd())
except TypeError as e:
    print("5_reassoc_unknown_ok", type(e).__name__)
print("5_reassoc_float", reassoc_float(1.0))


# 6. Dominator-tree-scoped CSE across branches (and non-raising preservation)
def cse_across_branches(n, flag):
    acc = 0
    for i in range(n):
        base = (i + 1) * 3
        if flag:
            acc += (i + 1) * 3 + base
        else:
            acc += (i + 1) * 3 - 1
    return acc


def div_zero_not_eliminated():
    try:
        _ = 1 // 0
        return "unreachable"
    except ZeroDivisionError:
        return "caught_div_zero"


print("6_cse", cse_across_branches(6, True), cse_across_branches(6, False), div_zero_not_eliminated())


# 7. Exception regions: try/except/finally inside loops & PEP 654 except*
def exc_in_loop(items):
    out = []
    for x in items:
        state = "init"
        try:
            state = "try"
            out.append(10 // x)
        except ZeroDivisionError:
            out.append(state)
        finally:
            out.append("fin")
    return out


def except_star_partial():
    caught = []
    residual = None
    try:
        try:
            raise ExceptionGroup("eg", [ValueError("v"), TypeError("t"), KeyError("k")])
        except* ValueError as eg_v:
            caught.append(("V", len(eg_v.exceptions)))
        except* KeyError as eg_k:
            caught.append(("K", len(eg_k.exceptions)))
    except ExceptionGroup as rem:
        residual = [type(e).__name__ for e in rem.exceptions]
    return (caught, residual)


print("7_exc_loop", exc_in_loop([2, 0, 5]))
print("7_except_star", except_star_partial())


# 8. Generators, yield from, closures/cells, and locals() observability
def sub_gen(n):
    for i in range(n):
        yield i * 10


def outer_gen(n):
    yield "start"
    yield from sub_gen(n)
    yield "end"


def make_adder(base):
    offset = base + 1

    def add(x):
        return x + offset

    return add


def inspect_locals(a, b):
    c = a + b
    if a > 0:
        c = c * 2
    snap1 = dict(sorted(locals().items()))
    snap2 = dict(sorted(sys._getframe().f_locals.items()))
    return (snap1, snap2)


print("8_gen", list(outer_gen(4)))
adder = make_adder(10)
print("8_closure", adder(5), adder(20))
print("8_locals", inspect_locals(3, 4), inspect_locals(-1, 4))
