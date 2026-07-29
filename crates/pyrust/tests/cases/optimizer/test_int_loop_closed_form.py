# Counted loops over a constant `range` fold to a closed form behind an
# exact-bounds iterator guard. Everything observable about the original loop
# must survive: the accumulator's value, the loop variable's final binding, the
# live module namespace, and the deopt to the original loop whenever the
# iterator is not the range the fold was built from.

# ── The folded shape itself ───────────────────────────────────────────────
total = 0
for i in range(50):
    total += 1
print("count", total, i)
print("count globals", globals()["total"], globals()["i"])

# The loop variable keeps the last value the range yielded, and stays writable.
i = i + 100
print("rebind", i)

# ── Sum of the loop variable, and 2/3-argument ranges ─────────────────────
s = 0
for j in range(10, 1000, 3):
    s += j
print("sum", s, j)

low = 0
for k in range(5, 12):
    low += k
print("two-arg", low, k)

# A negative step still yields a descending arithmetic series.
down = 0
for m in range(100, 0, -7):
    down += m
print("negative step", down, m)

# A step wider than the span yields exactly one value.
wide = 0
for n in range(0, 10, 100):
    wide += 1
print("wide step", wide, n)

# Mixed accumulators in one body, plus `-=`.
a = 0
b = 0
c = 100
for p in range(1, 6):
    a += 2
    b += p
    a += p
    c -= 3
print("mixed", a, b, c, p)

# ── Zero-trip ranges bind nothing ─────────────────────────────────────────
print("zero before", "zt" in globals())
for zt in range(0):
    zt = 999
print("zero after", "zt" in globals())

for zt3 in range(10, 10, 3):
    zt3 = 999
print("zero three-arg", "zt3" in globals())

# ── Exact integer semantics across the machine-int boundary ───────────────
# The fold applies one add; the original applied many. Both must promote to
# the same arbitrary-precision value.
ov = (2 ** 63) - 5
for q in range(10):
    ov += 1
print("overflow", ov, q)

big = 2 ** 70
for r in range(1000):
    big += 1
print("bigint accumulator", big, r)

# ── Deopt paths ───────────────────────────────────────────────────────────
# A non-int accumulator fails the entry guard and runs the original loop.
f = 0.5
for t in range(10):
    f += 1
print("float accumulator", f, t)

# Bounds beyond the machine-int cursor never reach the fold.
huge = 0
for u in range(2 ** 63 - 2, 2 ** 63 + 2):
    huge += 1
print("wide bounds", huge, u)

# A body that breaks is not a linear accumulation.
br = 0
for v in range(100):
    br += 1
    if br == 5:
        break
print("break", br, v)

# The inner loop of a nest is the one that folds.
outer = 0
for w in range(3):
    for x in range(5):
        outer += 1
print("nested", outer, w, x)


# ── Rebinding `range` mid-program must take the original path ─────────────
_builtin_range = range


def shadow_list(n):
    return [7, 8, 9]


range = shadow_list
sh = 0
for y in range(3):
    sh += y
print("shadowed list", sh, y)


# The sharpest case: a rebound `range` that still returns a genuine range, but
# with different bounds than the constant at the call site. A guard that only
# checked the *kind* of iterator would fold the wrong trip count here.
def shadow_range(n):
    return _builtin_range(5)


range = shadow_range
sh2 = 0
for z in range(1000):
    sh2 += 1
print("shadowed range", sh2, z)

range = _builtin_range
back = 0
for aa in range(4):
    back += 1
print("restored", back, aa)
