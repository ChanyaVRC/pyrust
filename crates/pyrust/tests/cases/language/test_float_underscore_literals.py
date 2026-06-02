# Parity fixture for PEP 515 underscore placement in numeric LITERALS
# (issue #2036). Distinct from int()/float() string conversion (#1896).
#
# An underscore in a decimal/float literal is valid ONLY between two digits:
# in the integer part, the fraction, and the exponent. Leading, trailing,
# doubled, and underscores adjacent to `.`, `e`/`E`, or a sign are SyntaxErrors.


def show(src):
    try:
        print(repr(src), "->", eval(src))
    except SyntaxError:
        print(repr(src), "-> SyntaxError")


# --- valid: underscore between digits in int part, fraction, exponent ---
show("1_0")
show("1_000")
show("1_0.0_1")
show("100_000.0")
show("1.0e1_0")
show("1e1_0")
show("1_0e2")
show("1.5e1_0")
show("1.2_3e4_5")
show("1_2_3.4_5e6_7")
show(".1_2")
show(".5e1_0")
show("1e5_0")
show("1_000j")
show("1_0.5j")

# --- invalid: misplaced underscores ---
show("1_.5")
show("1.5_")
show("1.0_")
show("1__0.5")
show("1.0_e5")
show("1._5")
show("1.e_5")
show("1e_5")
show("1e5_")
show("1e+_5")
show(".5_")
show("1_e5")

# --- regression: non-decimal prefixes keep their own underscore rules ---
show("0x_FF")
show("0o_17")
show("0b_11")
show("0x1_f")
show("0o7_7")
show("0b1_0")

# --- regression: plain floats and ints ---
show("1.5e10")
show("1e10")
show("0.5")
show("1.")
show(".5")
