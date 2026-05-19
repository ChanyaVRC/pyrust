# Parity fixture: float repr/str must match CPython 3.12's shortest-round-trip output.
# CPython uses David Gay's dtoa (Grisu/Dragon4 family); pyrust uses ryu.
# Both produce the same shortest decimal representation, and we normalise the
# exponent format to match CPython ("e+20", "e-05" — sign always present,
# exponent at least two digits).

# --- special values ---
print(repr(float('inf')))    # inf
print(repr(float('-inf')))   # -inf
print(repr(float('nan')))    # nan

# --- zero ---
print(repr(0.0))             # 0.0
print(repr(-0.0))            # -0.0

# --- integer-valued floats: always have decimal point ---
print(repr(1.0))             # 1.0
print(repr(100.0))           # 100.0
print(repr(1000.0))          # 1000.0
print(repr(1e15))            # 1000000000000000.0
print(repr(1e16))            # 1e+16
print(repr(1e17))            # 1e+17

# --- large values: scientific notation ---
print(repr(1e20))                       # 1e+20
print(repr(1.5e30))                     # 1.5e+30
print(repr(1.7976931348623157e+308))    # 1.7976931348623157e+308
print(repr(-1e20))                      # -1e+20

# --- values near 1e-4 threshold ---
print(repr(1e-4))            # 0.0001  (exactly at threshold, stays decimal)
print(repr(9.999e-5))        # 9.999e-05 (just below threshold, scientific)
print(repr(1e-5))            # 1e-05
print(repr(2e-5))            # 2e-05
print(repr(1.5e-5))          # 1.5e-05
print(repr(1e-10))           # 1e-10
print(repr(1e-100))          # 1e-100
print(repr(5e-324))          # 5e-324  (denormal minimum positive)
print(repr(-1e-5))           # -1e-05

# --- shortest round-trip: no unnecessary digits ---
print(repr(1.1))             # 1.1  (not 1.1000000000000001)
print(repr(1.5))             # 1.5
print(repr(0.1 + 0.2))      # 0.30000000000000004  (exact sum)

# --- str() and repr() agree for floats ---
print(str(1.5) == repr(1.5))    # True
print(str(1e20) == repr(1e20))  # True
print(str(1e-5) == repr(1e-5))  # True
