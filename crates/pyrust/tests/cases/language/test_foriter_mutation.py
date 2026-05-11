# for-loop over list with body that mutates length: pyrust used to panic
# with an index out-of-bounds.  CPython iterates by index and does NOT raise;
# removing items just ends iteration early.

# ── shrinking list: terminates without crash ───────────────────────────────
lst = [1, 2, 3, 4, 5]
seen = []
for x in lst:
    seen.append(x)
    lst.pop()   # shrink from end; CPython just stops when pos >= len

assert len(seen) <= 5   # no crash, some elements visited

# ── non-mutating iteration still gives correct total ──────────────────────
lst2 = [10, 20, 30]
total = 0
for x in lst2:
    total += x
assert total == 60

# ── tuple iteration unaffected ────────────────────────────────────────────
s = 0
for x in (1, 2, 3):
    s += x
assert s == 6

print("foriter ok")
