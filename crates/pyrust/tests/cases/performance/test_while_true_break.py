# Regression: `while True: if c: break; rest` must produce the same result
# as the equivalent `while not c: rest` form (issue #282).  The compiler
# rewrites the trampoline shape so the loop also picks up the canonical
# `ForCountConstInline` promotion path when the body is an inductive counter.

# (1) Break-at-top with literal int bound — exact ForCount* shape.
N = 100
total = 0
i = 0
while True:
    if i >= N:
        break
    total += i
    i += 1
expected = sum(range(N))
assert total == expected, (total, expected)
assert i == N, i  # post-loop counter value must match canonical while-form
print("while-true-break-1", total)

# (2) Break-at-top with non-literal bound (forces ForCountReg path).
limit = 50
acc = 0
k = 0
while True:
    if k >= limit:
        break
    acc += k * 2
    k += 1
expected_acc = sum([x * 2 for x in range(limit)])
assert acc == expected_acc, (acc, expected_acc)
assert k == limit, k
print("while-true-break-2", acc)

# (3) Break never fires (single iteration via outer condition) — body runs
# its full count and condition is exercised once-per-iteration.
total = 0
i = 0
while True:
    if i >= 10:
        break
    total += i
    i += 1
assert total == 45, total
print("while-true-break-3", total)

# (4) Break fires immediately (zero-iteration loop).
ran = False
n = 5
while True:
    if n > 0:
        break
    ran = True
assert ran is False, "body should not have executed"
print("while-true-break-4", ran)

# (5) Break-at-top inside a nested loop must target the inner loop only.
pairs = []
for i in range(3):
    j = 0
    while True:
        if j >= 3:
            break
        pairs.append((i, j))
        j += 1
assert len(pairs) == 9, pairs
assert pairs[0] == (0, 0) and pairs[-1] == (2, 2)
print("while-true-break-5", len(pairs))

# (6) Guard observed via an in-condition list.append (mutating method on a
# local list literal) — the rewrite must preserve guard evaluation count
# once per iteration including the final breaking iteration.
# trace.append returns None (falsy) so `or` falls through to the real test.
trace = []
total = 0
i = 0
while True:
    if trace.append(i) or i >= 5:
        break
    total += i
    i += 1
assert total == 0 + 1 + 2 + 3 + 4, total
# Guard runs once per iteration including the final iteration that breaks → 6 entries.
assert trace == [0, 1, 2, 3, 4, 5], trace
print("while-true-break-6", total, len(trace))

# (7) `while True: if c: break` with EMPTY rest — busy-spin shape.  The
# rewrite still applies; the loop becomes `while not c: pass` and exits
# the first time c is true.
toggle = True
while True:
    if toggle:
        break
assert toggle is True
print("while-true-break-7", toggle)

# (8) Body still contains a (different) `break` after the rewritten top.
# Inner break must still exit the loop correctly.
found = -1
i = 0
while True:
    if i >= 20:
        break
    if i * i == 49:
        found = i
        break
    i += 1
assert found == 7, found
print("while-true-break-8", found)
