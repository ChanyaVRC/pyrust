"""Parity: integer fast path in CmpJumpIfFalse/CmpJumpIfTrue/Const variants.

Covers all six comparison operators so that the as_int() fast path inside
the VM dispatch loop is exercised for each opcode shape.
"""

# ── CmpJumpIfFalse (while loop back-edge) ────────────────────────────────────

# Lt: i < n
i = 0
while i < 5:
    i += 1
print(i)  # 5

# Le: i <= n
i = 0
while i <= 4:
    i += 1
print(i)  # 5

# Gt: i > n
i = 5
while i > 0:
    i -= 1
print(i)  # 0

# Ge: i >= n
i = 5
while i >= 1:
    i -= 1
print(i)  # 0

# Eq: used as a while-loop condition (rare but valid)
i = 3
while i == 3:
    i = 4
print(i)  # 4

# Ne: i != n
i = 0
while i != 5:
    i += 1
print(i)  # 5

# ── CmpJumpIfTrue (if-statement taken / not-taken) ────────────────────────────

# These exercises CmpJumpIfTrue rather than CmpJumpIfFalse.
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

# ── CmpJumpIfFalseConst (compare register to constant) ───────────────────────

def count_up_to(n):
    """Uses CmpJumpIfFalseConst: `i < n` where n is a loop-invariant const."""
    i = 0
    s = 0
    while i < n:
        s += i
        i += 1
    return s

print(count_up_to(10))   # 45
print(count_up_to(0))    # 0
print(count_up_to(1))    # 0

# ── CmpJumpIfTrueConst ────────────────────────────────────────────────────────

def any_above(items, threshold):
    for x in items:
        if x > threshold:
            return True
    return False

print(any_above([1, 2, 3, 4, 5], 3))   # True
print(any_above([1, 2, 3], 10))        # False
print(any_above([], 0))                 # False

# ── Boundary values ───────────────────────────────────────────────────────────

# i64::MAX and i64::MIN can be represented as Python int;
# verify the fast path doesn't corrupt comparison results.
big = 9223372036854775807   # i64::MAX
small = -9223372036854775808  # i64::MIN

print(big > 0)        # True
print(small < 0)      # True
print(big != small)   # True
print(big == big)     # True
