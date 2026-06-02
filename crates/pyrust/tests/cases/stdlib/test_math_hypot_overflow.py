# Overflow/underflow-safe magnitude for math.hypot, math.dist, and abs(complex).
# Issues #1977 (hypot/dist) and #1982 (abs(complex)): the naive
# sqrt(sum-of-squares) overflowed to inf (or underflowed to 0.0) for very
# large / very small components.  CPython uses a scaled, compensated norm.
import math

inf = float("inf")
nan = float("nan")

# --- hypot: overflow / underflow ---
print(math.hypot(3e300, 4e300))      # 5e+300, not inf
print(math.hypot(3e-300, 4e-300))    # 5e-300, not 0.0
print(math.hypot(1e308, 1e308))      # ~1.414e+308, not inf
print(math.hypot(5e300))             # 5e+300 (1 arg)
print(math.hypot())                  # 0.0 (no args)
print(math.hypot(3, 4))              # 5.0
print(math.hypot(3, 4, 12))          # 13.0 (n > 2)
print(math.hypot(-3, -4))            # 5.0 (negatives)
print(math.hypot(0, 0, 0))           # 0.0

# --- hypot: inf / nan rules (inf beats nan) ---
print(math.hypot(inf, nan))          # inf
print(math.hypot(nan, inf))          # inf
print(math.hypot(inf, 3))            # inf
print(math.hypot(nan, 3))            # nan

# --- hypot: mixed huge / tiny ---
print(math.hypot(1e-200, 1e200))     # 1e+200

# --- dist ---
print(math.dist([0, 0], [3e300, 4e300]))   # 5e+300, not inf
print(math.dist([1, 2, 3], [4, 5, 6]))     # 5.196152422706632
print(math.dist([0.0], [0.0]))             # 0.0
print(math.dist((1, 1), (1, 1)))           # 0.0

# dist length validation is unchanged.
try:
    math.dist([1, 2], [1])
except ValueError as e:
    print("ValueError:", e)

# --- abs(complex): overflow / underflow ---
print(abs(complex(1e308, 1e308)))    # ~1.414e+308, not inf
print(abs(complex(3e300, 4e300)))    # 5e+300, not inf
print(abs(complex(3, 4)))            # 5.0
print(abs(complex(0, 0)))            # 0.0
print(abs(complex(inf, 1)))          # inf
print(abs(complex(1, inf)))          # inf
print(abs(complex(nan, 1)))          # nan
print(abs(complex(inf, nan)))        # inf (inf beats nan)

# --- abs of plain numbers is unchanged ---
print(abs(-5))
print(abs(-2.5))
print(abs(True))
