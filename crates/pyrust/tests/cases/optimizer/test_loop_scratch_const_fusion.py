# Issue #2889.
#
# A `while COND: … EXPR == K …` loop body reloads a single scratch temp with a
# different constant for each operand.  The fusion pass now proves that scratch
# value dead after its consumer, so the loop bound's own `LoadConst` folds into
# the comparison and the back-edge lands on the loop header again — which is
# what lets loop inversion and int-loop versioning rewrite the loop.
#
# Everything below must keep its exact CPython semantics through that rewrite:
# operand order, reflected operators, non-int operands falling off the guarded
# fast path, and the value each loop computes.

# The shape from bench/cases/continue_top.py, at module scope.
i = 0
total = 0
while i < 20:
    if i % 2 == 0:
        i += 1
        continue
    total += i
    i += 1
print("continue-at-top:", i, total)

# The manually inverted equivalent must agree.
i = 0
total = 0
while i < 20:
    if i % 2 != 0:
        total += i
    i += 1
print("inverted:", i, total)

# Zero-iteration loop: the header's fused comparison is the only thing that
# runs, and the body's constants are never loaded.
i = 100
total = 0
while i < 20:
    if i % 2 == 0:
        i += 1
        continue
    total += i
    i += 1
print("zero iterations:", i, total)

# Same shape inside a function, where the registers are fast locals.
def sum_odd(n):
    i = 0
    total = 0
    while i < n:
        if i % 2 == 0:
            i += 1
            continue
        total += i
        i += 1
    return total


print("in a function:", sum_odd(0), sum_odd(1), sum_odd(21))


# Boundary operands: the guarded int version must not be entered when the loop
# variable leaves the machine-int range, and the result must still be exact.
i = (1 << 62) - 4
total = 0
while i < (1 << 62) + 4:
    if i % 2 == 0:
        i += 1
        continue
    total += i
    i += 1
print("across the i64 boundary:", i, total)

# Negative operands keep Python's floor-mod semantics (`-3 % 2 == 1`).
i = -6
seen = []
while i < 6:
    if i % 2 == 0:
        i += 1
        continue
    seen.append(i)
    i += 1
print("negative:", seen)


# The scratch temp feeds a user-defined type: fusing must keep the constant on
# the right-hand side, so `__eq__`/`__mod__` are called and never their
# reflected counterparts with swapped operands.
class Traced:
    def __init__(self, value):
        self.value = value

    def __mod__(self, other):
        print("  __mod__", self.value, other)
        return Traced(self.value % other)

    def __rmod__(self, other):
        print("  __rmod__ (must not run)", other, self.value)
        return NotImplemented

    def __eq__(self, other):
        print("  __eq__", self.value, other)
        return self.value == other

    def __lt__(self, other):
        return self.value < other

    def __hash__(self):
        return hash(self.value)


print("user type:")
t = Traced(7)
print("  result:", t % 4 == 3)

# A loop whose comparison operands are not ints falls back to the generic copy.
values = [Traced(1), Traced(2), Traced(3)]
k = 0
kept = []
while k < len(values):
    if values[k] % 2 == 0:
        k += 1
        continue
    kept.append(values[k].value)
    k += 1
print("  kept:", kept)


# `continue` inside a nested loop must still target its own header.
rows = []
outer = 0
while outer < 4:
    inner = 0
    row = []
    while inner < 4:
        if inner % 2 == 0:
            inner += 1
            continue
        row.append(outer * inner)
        inner += 1
    rows.append(row)
    outer += 1
print("nested:", rows)


# A raising bare expression statement inside the fused shape must still report
# its own source line (the fused op keeps its origin's line table entry).
import sys

_big = 10**400
_x = _big
try:
    _x / 2
except OverflowError:
    print("bare binop line:", sys.exc_info()[2].tb_lineno)
