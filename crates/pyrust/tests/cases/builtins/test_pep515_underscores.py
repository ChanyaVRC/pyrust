# Parity fixture for PEP 515 underscore handling in int()/float() string
# constructors (issue #1896).
#
# An underscore is valid ONLY between two digits, with the single exception
# that it may immediately follow a base prefix (0x/0o/0b) in int(). Leading,
# trailing, doubled, and digit-non-adjacent underscores are ValueErrors.


def show_int(s, base):
    try:
        print(repr(s), base, int(s, base))
    except ValueError as e:
        print(repr(s), base, f"ValueError: {e}")


def show_int1(s):
    try:
        print(repr(s), int(s))
    except ValueError as e:
        print(repr(s), f"ValueError: {e}")


def show_float(s):
    try:
        print(repr(s), float(s))
    except ValueError as e:
        print(repr(s), f"ValueError: {e}")


# --- int(): valid between-digit underscores ---
show_int1("1_000")
show_int1("1_0_0")
show_int1("10_000_000")
show_int1("+1_0")
show_int1("-1_0")
show_int1("  1_0  ")

# --- int() explicit base: post-prefix underscore is valid ---
show_int("0x_FF", 16)
show_int("0o_17", 8)
show_int("0b_101", 2)
show_int("0xFF_FF", 16)
show_int("0X_ff", 16)
show_int("FF_FF", 16)
show_int("1_0", 16)
show_int("-0x_FF", 16)
show_int("+0x_FF", 16)

# --- int() base 0: prefix + post-prefix underscore ---
show_int("0x_FF", 0)
show_int("0o_17", 0)
show_int("0b_101", 0)
show_int("1_0", 0)
show_int("0_0", 0)
show_int("00_0", 0)
show_int("-0x_FF", 0)

# --- int(): invalid placements (all ValueError) ---
show_int1("1_")
show_int1("_1")
show_int1("1__0")
show_int1("_")
show_int1("-_")
show_int("0x__FF", 16)
show_int("_FF", 16)
show_int("0x_", 16)
show_int("0x_", 0)
show_int("0_x1", 0)

# --- int(): BigInt-magnitude values with underscores ---
show_int1("123_456_789_012_345_678_901_234_567_890")
show_int("0x_FFFF_FFFF_FFFF_FFFF_FFFF", 16)

# --- float(): valid underscores in mantissa and exponent ---
show_float("1_000.5")
show_float("1_0e1_0")
show_float("1.5_0")
show_float("1_000.000_1")
show_float("1_0.5_0")
show_float("+1_0.5")
show_float("-1_0.0")
show_float("  1_0.5  ")
show_float("1_2_3.4_5e6_7")
show_float("1_000")

# --- float(): invalid placements (all ValueError) ---
show_float("1_")
show_float("_1")
show_float("1__0")
show_float("1_.0")
show_float("1._5")
show_float("1_e5")
show_float("1e_5")
show_float("1e5_")
show_float("_")
show_float("1_0e1_")
show_float("1_0e_1")

# --- float(): special strings are unaffected by underscore handling ---
show_float("inf")
show_float("nan")
show_float("-inf")
