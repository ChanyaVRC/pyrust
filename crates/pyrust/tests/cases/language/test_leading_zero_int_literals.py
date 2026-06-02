# Parity fixture for leading-zero decimal integer literals (issue #2033).
#
# Python 3 forbids a leading zero in a decimal integer literal unless every
# digit is zero. `0`, `00`, `0_0` are valid (== 0); `0123`, `09`, `0_1` are a
# SyntaxError. The 0x/0o/0b prefixes, floats (0.5/0e0), and complex (0j/01j)
# are all exempt.


def show(src):
    try:
        print(repr(src), "->", eval(src))
    except SyntaxError:
        print(repr(src), "-> SyntaxError")


# --- valid all-zero decimal integers ---
show("0")
show("00")
show("000")
show("0_0")
show("0_0_0")

# --- invalid: leading zero followed by a nonzero digit ---
show("0123")
show("09")
show("01")
show("0_1")
show("007")
show("00_1")
show("0_00_1")

# --- exempt: non-decimal prefixes ---
show("0o17")
show("0x1F")
show("0b11")
show("0o0")
show("0x0")

# --- exempt: floats and complex starting with 0 ---
show("0.5")
show("0e0")
show("00.5")
show("0j")
show("00j")
show("01j")
show("0_0j")

# --- regular decimals still lex ---
show("10")
show("100")
show("1_0")
show("1_000")
