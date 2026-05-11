t = (3, 1, 4, 1, 5)

# index
assert t.index(4) == 2
assert t.index(1) == 1
assert t.index(1, 2) == 3

# count
assert t.count(1) == 2
assert t.count(9) == 0

print("tuple methods OK")

# ── tuple concatenation (Issue #102) ──────────────────────────────────────
print("tuple-concat", (1, 2) + (3, 4))
print("tuple-concat-empty", () + (1,))
print("tuple-concat-left-empty", (1, 2) + ())
print("tuple-concat-chain", (1,) + (2,) + (3,))
a = (1, 2)
b = (3, 4)
print("tuple-concat-vars", a + b)
