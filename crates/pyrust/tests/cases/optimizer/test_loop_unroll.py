# pass_forcount_unroll: small constant-trip loops are unrolled at compile time.
#
# A ForCountConstInline loop whose trip count is <= 4 and whose total unrolled
# body size (body_insns * trip) is <= 32 is replaced with N copies of the body,
# each preceded by a LoadConst that sets the loop variable to that iteration's
# value.  The result must be identical to CPython 3.12.

# ── Basic case: range(4), trip = 4 ──────────────────────────────────────────
# This is the canonical unrollable loop.  Output: 0 1 2 3 on separate lines.
for i in range(4):
    print(i)

# ── Verify loop variable retains last value post-loop ────────────────────────
x = 0
for j in range(4):
    x += j
assert x == 6, x
assert j == 3, j
print("sum4", x, j)

# ── trip = 1: single-iteration loop ─────────────────────────────────────────
total = 0
for _ in range(1):
    total += 99
assert total == 99, total
print("trip1", total)

# ── trip = 4 with step = 2 ──────────────────────────────────────────────────
# range(0, 8, 2) → 0, 2, 4, 6 (trip = 4)
for k in range(0, 8, 2):
    print(k)

# ── Backward loop: step = -1, trip = 3 ──────────────────────────────────────
for i in range(2, -1, -1):
    print(i)

# ── trip = 5 should NOT be unrolled (above threshold) ───────────────────────
# Correctness only: must still produce the right answer whether or not
# unrolling fires.
total5 = 0
for i in range(5):
    total5 += i
assert total5 == 10, total5
print("nounroll5", total5)

# ── Loop with break should NOT be unrolled ───────────────────────────────────
# Output: 0 (only; breaks at i == 1)
for i in range(3):
    if i == 1:
        break
    print(i)

# ── Loop with continue should NOT be unrolled ───────────────────────────────
# Output: 0 2 (skip i == 1)
for i in range(3):
    if i == 1:
        continue
    print(i)

# ── Zero-trip loop: no output, variable not written ─────────────────────────
ran = False
for i in range(0):
    ran = True
assert not ran, ran
print("zero_trip_ok")

# ── Body with if/else (intra-body jump) ─────────────────────────────────────
# Both branches stay within the body — unrolling should fire.
for i in range(4):
    if i % 2 == 0:
        print("even", i)
    else:
        print("odd", i)
