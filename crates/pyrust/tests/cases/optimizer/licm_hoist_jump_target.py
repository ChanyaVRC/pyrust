# Regression for #323: LICM must redirect in-loop jumps that targeted a hoisted
# instruction.  The `while True: if k >= var_bound: break; <body with k*literal>`
# shape hoists the literal `LoadConst` out, but the `CmpJumpIfFalse` for the
# break-test still pointed at the hoisted slot, looping back into the
# pre-header and hanging.
#
# Trigger matrix from the issue: var bound + var*literal body.

# 1. The canonical reproducer.
limit = 3
k = 0
while True:
    if k >= limit:
        break
    z = k * 2
    k += 1
assert k == 3, k
assert z == 4, z

# 2. The accumulator variant — same shape, different body.
limit = 5
k = 0
acc = 0
while True:
    if k >= limit:
        break
    acc += k * 2
    k += 1
assert k == 5, k
assert acc == 20, acc  # 0+2+4+6+8

# 3. The non-`augassign` variant.
limit = 4
k = 0
acc = 0
while True:
    if k >= limit:
        break
    acc = acc + k * 2
    k += 1
assert k == 4, k
assert acc == 12, acc  # 0+2+4+6

# 4. Literal bound (was already OK pre-fix) — ensure no regression.
k = 0
while True:
    if k >= 5:
        break
    z = k * 3
    k += 1
assert k == 5, k
assert z == 12, z

# 5. Canonical `while cond:` (always OK) — ensure unchanged.
limit = 3
k = 0
while k < limit:
    z = k * 2
    k += 1
assert k == 3, k
assert z == 4, z

# 6. `acc += 2 * k` (literal-first) — was already OK because the BinOpConst
#    isn't recognised by LICM as hoistable.  Pin it.
limit = 4
k = 0
acc = 0
while True:
    if k >= limit:
        break
    acc += 2 * k
    k += 1
assert k == 4, k
assert acc == 12, acc

print("licm_hoist_jump_target ok")
