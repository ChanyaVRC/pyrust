import math

# ndigits < 0: negative float rounding to zero must preserve sign (IEEE 754 -0.0)
print(repr(round(-1.5, -100)))    # -0.0
print(repr(round(-0.5, -100)))    # -0.0
print(repr(round(-7.5, -308)))    # -0.0
print(repr(round(-0.0, -308)))    # -0.0
print(repr(round(-1e-300, -308))) # -0.0

# Positive float rounds to +0.0 (sign preserved, not flipped)
print(repr(round(7.5, -308)))     # 0.0
print(repr(round(1.5, -100)))     # 0.0

# Non-zero results: sign comes from rounding, not forced
print(repr(round(-1.5, -1)))      # -0.0 (rounds to 0, sign preserved)
print(repr(round(-15.0, -1)))     # -20.0 (non-zero: half-even rounds away)
print(repr(round(-25.0, -1)))     # -20.0 (non-zero: half-even rounds toward even)
print(repr(round(-1.5, 0)))       # -2.0

# ndigits >= 0: negative float rounding to zero also preserves sign
print(repr(round(-0.001, 2)))     # -0.0
print(repr(round(-0.3, 0)))       # -0.0
print(repr(round(-0.4, 0)))       # -0.0

# Verify sign using math.copysign to distinguish 0.0 from -0.0
def check_sign(val, expected_neg):
    sign = math.copysign(1.0, val)
    expected_sign = -1.0 if expected_neg else 1.0
    print(sign == expected_sign)

check_sign(round(-1.5, -100), expected_neg=True)
check_sign(round(1.5, -100), expected_neg=False)
check_sign(round(-0.0, -308), expected_neg=True)
check_sign(round(-0.001, 2), expected_neg=True)
check_sign(round(-0.3, 0), expected_neg=True)
