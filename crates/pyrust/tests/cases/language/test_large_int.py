# Integers that exceed i48 range (±140_737_488_355_327) exercise the
# Opaque::BigInt fallback path in NaN boxing.

I48_MAX = 140_737_488_355_327
I48_MIN = -140_737_488_355_328

# ── boundary: values just at the i48 edge ──────────────────────────────────
assert I48_MAX + 0 == 140_737_488_355_327
assert I48_MIN + 0 == -140_737_488_355_328

# ── one step beyond i48 → Opaque::BigInt ──────────────────────────────────
big = I48_MAX + 1          # 140_737_488_355_328
assert big == 140_737_488_355_328
assert big > I48_MAX

neg_big = I48_MIN - 1      # -140_737_488_355_329
assert neg_big == -140_737_488_355_329
assert neg_big < I48_MIN

# ── i64 extremes ──────────────────────────────────────────────────────────
# Express as calculations to avoid literals that exceed the lexer's i64 range.
I64_MAX = 2 ** 63 - 1
I64_MIN = -(2 ** 63)

assert I64_MAX == 2 ** 63 - 1
assert I64_MIN == -(2 ** 63)
assert I64_MAX + I64_MIN == -1

# ── arithmetic across the i48 boundary ────────────────────────────────────
a = I48_MAX
a += 1
assert a == 140_737_488_355_328   # now BigInt

a += I48_MAX
assert a == 281_474_976_710_655

a -= I48_MAX
assert a == 140_737_488_355_328   # back to BigInt

a -= 1
assert a == I48_MAX               # back to inline i48

# ── multiplication ─────────────────────────────────────────────────────────
assert 10 ** 15 == 1_000_000_000_000_000
assert 10 ** 18 == 1_000_000_000_000_000_000
assert 2 ** 62  == 4_611_686_018_427_387_904

# ── comparisons mixing inline and BigInt ───────────────────────────────────
assert I48_MAX < I48_MAX + 1
assert I48_MIN > I48_MIN - 1
assert I48_MAX + 1 == I48_MAX + 1

# ── BigInt in collections ──────────────────────────────────────────────────
lst = [I64_MAX, I64_MIN, I48_MAX + 1]
assert lst[0] == I64_MAX
assert lst[1] == I64_MIN
assert lst[2] == 140_737_488_355_328
assert len(lst) == 3

d = {I64_MAX: "big", I48_MAX + 1: "medium"}
assert d[I64_MAX] == "big"
assert d[140_737_488_355_328] == "medium"

# ── int/float comparison (Python: 1 == 1.0) ───────────────────────────────
assert 1 == 1.0
assert 1.0 == 1
assert I48_MAX == float(I48_MAX)

print("large int OK")
