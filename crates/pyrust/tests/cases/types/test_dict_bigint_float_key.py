# CPython parity: BigInt values beyond i64 range must be treated as the same
# dict/set key as the corresponding integer-valued float.
#
# Root cause: PR #554 fixed Float <-> Int parity; this fixture covers
# Float <-> BigInt (issue #558).

# ── Basic: float key, then bigint key collides ────────────────────────────────
d = {1e20: 'a'}
d[10**20] = 'b'
print(len(d))       # 1
print(d[1e20])      # b  (bigint write overwrote float entry)
print(d[10**20])    # b

# ── Reverse order: bigint key first ───────────────────────────────────────────
d2 = {10**20: 'x'}
d2[1e20] = 'y'
print(len(d2))      # 1
print(d2[10**20])   # y

# ── Membership ────────────────────────────────────────────────────────────────
print(10**20 in {1e20: 'a'})   # True
print(1e20 in {10**20: 'a'})   # True

# ── Hash equality ─────────────────────────────────────────────────────────────
print(hash(1e20) == hash(10**20))   # True

# ── Set operations ────────────────────────────────────────────────────────────
s = {1e20, 10**20}
print(len(s))   # 1

s2 = {10**20}
print(1e20 in s2)   # True

# ── frozenset ─────────────────────────────────────────────────────────────────
fs = frozenset([1e20, 10**20])
print(len(fs))  # 1

# ── Distinct values (different integer) ───────────────────────────────────────
d3 = {1e20: 'a', 10**21: 'b'}
print(len(d3))  # 2

# ── inf vs large BigInt: should be distinct (10**400 != float('inf')) ─────────
d4 = {float('inf'): 'a'}
d4[10**400] = 'b'
print(len(d4))  # 2

# ── 2**53 boundary: float(2**53) is exact ────────────────────────────────────
d5 = {2**53: 'a', float(2**53): 'b'}
print(len(d5))  # 1

# ── Negative BigInt ───────────────────────────────────────────────────────────
d6 = {-(10**20): 'a', float(-(10**20)): 'b'}
print(len(d6))  # 1

# ── Regression: existing Float/Int parity still works ────────────────────────
d7 = {1: 'a', 1.0: 'b'}
print(len(d7))  # 1

# ── dict.keys() round-trip: BigInt key retrieves as int ──────────────────────
d8 = {10**20: 'val'}
keys = list(d8.keys())
print(keys[0])                    # 100000000000000000000
print(type(keys[0]).__name__)     # int
