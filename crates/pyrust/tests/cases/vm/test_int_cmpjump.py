"""Parity: integer fast path in CmpJumpIfFalse/CmpJumpIfFalseConst/CmpJumpIfTrue/CmpJumpIfTrueConst variants.

Covers all six comparison operators so that the as_int() fast path inside
the VM dispatch loop is exercised for each opcode shape.

Opcode shapes used in this fixture:
- CmpJumpIfFalse      -- while loop back-edge (non-range form) and if-statement
- CmpJumpIfFalseConst -- while loop with an Int literal on the RHS (BinOpConst path)
- CmpJumpIfTrue       -- assert statement with register RHS (emits JumpIfTrue)
- CmpJumpIfTrueConst  -- assert statement with an Int literal RHS (BinOpConst path)
"""

# ── CmpJumpIfFalse (while loop back-edge) ────────────────────────────────────
# Uses `i = i + 1` (Assign) rather than `i += 1` (AugAssign) so the
# compiler's try_compile_while_range optimisation does not fire and the loop
# back-edge remains a CmpJumpIfFalse* instruction.

# Lt: i < n
i = 0
while i < 5:
    i = i + 1
print(i)  # 5

# Le: i <= n
i = 0
while i <= 4:
    i = i + 1
print(i)  # 5

# Gt: i > n
i = 5
while i > 0:
    i = i - 1
print(i)  # 0

# Ge: i >= n
i = 5
while i >= 1:
    i = i - 1
print(i)  # 0

# Eq: used as a while-loop condition (rare but valid)
i = 3
while i == 3:
    i = 4
print(i)  # 4

# Ne: i != n
i = 0
while i != 5:
    i = i + 1
print(i)  # 5

# ── CmpJumpIfFalse (if-statement, body skipped when condition is false) ───────
# compile_if emits JumpIfFalse, so BinOp+JumpIfFalse fuses to CmpJumpIfFalse.
# This exercises CmpJumpIfFalse for each operator via the if-branch path.
x = 7
if x > 5:
    print("gt-ok")     # gt-ok
if x >= 7:
    print("ge-ok")     # ge-ok
if x < 10:
    print("lt-ok")     # lt-ok
if x <= 7:
    print("le-ok")     # le-ok
if x == 7:
    print("eq-ok")     # eq-ok
if x != 6:
    print("ne-ok")     # ne-ok

# ── CmpJumpIfFalse (register vs register, inside function) ───────────────────
# `while i < n` where n is a parameter (register), body avoids the
# try_compile_while_range pattern by using Assign instead of AugAssign.

def sum_range(n):
    """CmpJumpIfFalse: while i < n where n is a runtime register."""
    i = 0
    s = 0
    while i < n:
        s = s + i
        i = i + 1
    return s

print(sum_range(10))   # 45
print(sum_range(0))    # 0
print(sum_range(1))    # 0

# ── CmpJumpIfFalseConst (register vs compile-time constant) ──────────────────
# `while i < 10` with Int literal on the RHS produces BinOpConst, which fuses
# with JumpIfFalse to CmpJumpIfFalseConst.  The body puts the accumulate step
# last so that try_compile_while_range does not recognise the loop (it requires
# VAR += step to be the final statement).

def sum_up_to_10():
    """CmpJumpIfFalseConst: while i < 10 with literal RHS."""
    i = 0
    s = 0
    while i < 10:
        i = i + 1
        s = s + i
    return s

print(sum_up_to_10())  # 55

# ── CmpJumpIfTrue / CmpJumpIfTrueConst (assert statement) ────────────────────
# `assert expr` emits JumpIfTrue for the passing branch, so
# BinOp + JumpIfTrue fuses to CmpJumpIfTrue, and
# BinOpConst + JumpIfTrue fuses to CmpJumpIfTrueConst.

y = 10
# CmpJumpIfTrueConst: RHS is an Int literal → BinOpConst shape
assert y > 5
assert y >= 10
assert y < 20
assert y <= 10
assert y == 10
assert y != 9

def check_above(val, threshold):
    """CmpJumpIfTrue: RHS is a register (non-const threshold)."""
    assert val > threshold

check_above(7, 3)
check_above(10, 9)
print("assertions ok")  # assertions ok

# ── Boundary values ───────────────────────────────────────────────────────────

# i64::MAX and i64::MIN can be represented as Python int;
# verify the fast path doesn't corrupt comparison results.
big = 9223372036854775807   # i64::MAX
small = -9223372036854775808  # i64::MIN

print(big > 0)        # True
print(small < 0)      # True
print(big != small)   # True
print(big == big)     # True
