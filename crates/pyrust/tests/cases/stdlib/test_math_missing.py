# Parity tests for math module functions added in issue #649.
# Covers: copysign, isfinite, gcd, factorial, lcm, comb, perm, prod, hypot, dist.
import math

# ── copysign ──────────────────────────────────────────────────────────────────
print(math.copysign(1.0, -1.0))    # -1.0
print(math.copysign(1.0, -0.0))    # -1.0
print(math.copysign(-1.0, 1.0))    # 1.0
print(math.copysign(3.5, 0.0))     # 3.5

# ── isfinite ──────────────────────────────────────────────────────────────────
print(math.isfinite(1.0))          # True
print(math.isfinite(float('inf'))) # False
print(math.isfinite(float('nan'))) # False
print(math.isfinite(0.0))          # True

# ── gcd ───────────────────────────────────────────────────────────────────────
print(math.gcd(12, 8))            # 4
print(math.gcd(0, 5))             # 5
print(math.gcd(-4, 6))            # 2  (always non-negative)
print(math.gcd())                 # 0
print(math.gcd(7))                # 7
print(math.gcd(12, 8, 6))        # 2
print(math.gcd(0, 0))            # 0
print(math.gcd(True, 4))         # 1  (bool is subclass of int)

# ── factorial ─────────────────────────────────────────────────────────────────
print(math.factorial(0))          # 1
print(math.factorial(1))          # 1
print(math.factorial(5))          # 120
print(math.factorial(20))         # 2432902008176640000
print(math.factorial(True))       # 1  (bool is subclass of int)

try:
    math.factorial(-1)
except ValueError as e:
    print("factorial(-1): ValueError:", e)

try:
    math.factorial(1.5)
except TypeError as e:
    print("factorial(1.5): TypeError:", e)

try:
    math.factorial(1.0)
except TypeError as e:
    print("factorial(1.0): TypeError:", e)

# ── lcm ───────────────────────────────────────────────────────────────────────
print(math.lcm(4, 6))             # 12
print(math.lcm(0, 5))             # 0
print(math.lcm())                 # 1
print(math.lcm(7))                # 7
print(math.lcm(0, 0))            # 0
print(math.lcm(True, 6))         # 6  (bool is subclass of int)

# ── comb ──────────────────────────────────────────────────────────────────────
print(math.comb(5, 2))            # 10
print(math.comb(5, 0))            # 1
print(math.comb(5, 5))            # 1
print(math.comb(5, 6))            # 0  (k > n → 0)
print(math.comb(0, 0))           # 1

try:
    math.comb(-1, 2)
except ValueError as e:
    print("comb(-1, 2): ValueError:", e)

try:
    math.comb(5, -1)
except ValueError as e:
    print("comb(5, -1): ValueError:", e)

# ── perm ──────────────────────────────────────────────────────────────────────
print(math.perm(5, 2))            # 20
print(math.perm(5))               # 120
print(math.perm(5, 0))            # 1
print(math.perm(5, 5))            # 120
print(math.perm(3, 5))            # 0  (k > n → 0)

try:
    math.perm(-1)
except ValueError as e:
    print("perm(-1): ValueError:", e)

try:
    math.perm(5, -1)
except ValueError as e:
    print("perm(5, -1): ValueError:", e)

# ── prod ──────────────────────────────────────────────────────────────────────
print(math.prod([1, 2, 3, 4]))    # 24
print(math.prod([]))              # 1
print(math.prod([2, 3], start=10)) # 60
print(math.prod([True, 2, 3]))    # 6

# ── hypot ─────────────────────────────────────────────────────────────────────
print(math.hypot(3, 4))           # 5.0
print(math.hypot(0))              # 0.0
print(math.hypot())               # 0.0
print(math.hypot(1, 1, 1))       # 1.7320508075688772

# ── dist ──────────────────────────────────────────────────────────────────────
print(math.dist([0, 0], [3, 4])) # 5.0
print(math.dist([1], [1]))        # 0.0

try:
    math.dist([1, 2], [1, 2, 3])
except ValueError as e:
    print("dist mismatch: ValueError:", e)
