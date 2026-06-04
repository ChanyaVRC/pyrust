# Issue #2088: `and`/`or` short-circuit dropped when the RHS references an outer
# loop variable.  The trailing conditional jump of the RHS comparison was the
# target of the short-circuit's LHS-false jump; fusing that BinOp+JumpIfFalse
# into a CmpJump made the landing instruction recompute the RHS instead of
# re-testing the (false) LHS result, so the body ran even when the LHS was false.

# comprehension form (RHS references outer var x)
print([(x, y) for x in range(3) for y in range(3) if y > 0 and y != x])

# operand order swapped (outer-var ref on the LHS) — must match
print([(x, y) for x in range(3) for y in range(3) if y != x and y > 0])

# hand-written nested for + if (no comprehension)
res = []
for x in range(3):
    for y in range(3):
        if y > 0 and y != x:
            res.append((x, y))
print(res)

# or-chain short-circuit with outer-var RHS
print([(x, y) for x in range(3) for y in range(3) if y == 0 or y == x])

# triple nesting with two cross-scope conjuncts
print(
    [
        (a, b, c)
        for a in range(2)
        for b in range(2)
        for c in range(2)
        if c > 0 and c != a and b != a
    ]
)

# mixed and/or with outer var on both sides
out = []
for i in range(4):
    for j in range(4):
        if j > 0 and (j == i or j == i + 1):
            out.append((i, j))
print(out)
