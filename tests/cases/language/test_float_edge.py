# Float edge cases: NaN boxing normalises NaN to a canonical bit pattern,
# so float behaviour must still match CPython.

import math

# ── basic float arithmetic ─────────────────────────────────────────────────
assert 0.1 + 0.2 == 0.30000000000000004   # IEEE 754 rounding
assert 1.0 / 3.0 == 0.3333333333333333

# ── infinity ───────────────────────────────────────────────────────────────
inf = float("inf")
assert inf > 10 ** 18
assert -inf < -(10 ** 18)
assert inf + 1 == inf
assert math.isinf(inf)

# ── NaN ────────────────────────────────────────────────────────────────────
nan = float("nan")
assert math.isnan(nan)
assert not (nan == nan)   # NaN != NaN
assert not (nan < 0)
assert not (nan > 0)

# NaN produced by arithmetic should also behave as NaN
nan2 = inf - inf
assert math.isnan(nan2)
assert not (nan2 == nan2)

# ── int / float coercion ───────────────────────────────────────────────────
assert 2 ** 53 == float(2 ** 53)          # exact in IEEE 754
# ── float in collections ───────────────────────────────────────────────────
lst = [1.5, float("inf"), float("nan"), -0.0]
assert lst[0] == 1.5
assert math.isinf(lst[1])
assert math.isnan(lst[2])
assert lst[3] == 0.0   # -0.0 == 0.0

print("float edge OK")

# ── nan/inf repr (Issue #100) ──────────────────────────────────────────────
print("nan-repr", float("nan"))
print("inf-repr", float("inf"))
print("neginf-repr", float("-inf"))
print("nan-str", str(float("nan")))
print("inf-str", str(float("inf")))
print("neginf-str", str(float("-inf")))

# repr in collections
print("nan-in-list", [float("nan"), float("inf"), float("-inf")])
print("nan-as-key", {float("nan"): 1})

# arithmetic-produced nan/inf also repr correctly
print("arith-inf", 1e308 * 10)
print("arith-nan", float("inf") - float("inf"))
