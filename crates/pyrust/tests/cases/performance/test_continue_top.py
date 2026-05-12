# Regression: `if guard: continue` at the top of a loop must produce the same
# result as the inverted `if not guard: body` form (issue #287). The compiler
# rewrites the trampoline shape to lift the tail into the inverted branch.

# (1) Single-statement Continue body — `if x % 5 == 0: continue` then body.
total = 0
for x in range(100):
    if x % 5 == 0:
        continue
    total += x
expected = 0
for k in range(100):
    if k % 5 != 0:
        expected += k
assert total == expected, total
print("continue-top-1", total)

# (2) Continue preceded by an in-if increment (pre-continue tail).
total = 0
i = 0
while i < 100:
    if i % 3 == 0:
        i += 1
        continue
    total += i
    i += 1
expected = 0
for k in range(100):
    if k % 3 != 0:
        expected += k
assert total == expected, total
print("continue-top-2", total)

# (3) Two consecutive if-continues at top — both must be folded.
out = []
for x in range(20):
    if x < 5:
        continue
    if x >= 15:
        continue
    out.append(x)
assert out == list(range(5, 15)), out
print("continue-top-3", out)

# (4) Continue inside nested loop (must target the inner loop only).
pairs = []
for i in range(4):
    for j in range(4):
        if i == j:
            continue
        pairs.append((i, j))
assert len(pairs) == 12, pairs
print("continue-top-4", len(pairs))

# (5) Continue at end of body with no tail — must NOT mis-fold.
seen = []
for x in range(5):
    seen.append(x)
    if x == 2:
        continue
assert seen == [0, 1, 2, 3, 4], seen
print("continue-top-5", seen)

# (6) Generic while (no augassign tail) — slow-path branch in compile_while.
acc = 0
items = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
idx = 0
while idx < len(items):
    cur = items[idx]
    idx += 1
    if cur % 2 == 0:
        continue
    acc += cur
assert acc == 1 + 3 + 5 + 7 + 9, acc
print("continue-top-6", acc)
