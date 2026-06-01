# Parity fixture for issue #1897: complex __format__ with presentation types.
#
# CPython applies the float format mini-language (type f/F/e/E/g/G/n,
# precision, sign, alt, width, fill/align) to both the real and imaginary
# parts of a complex, joining them as <re><signed-im>j.  pyrust previously
# rejected every presentation type / precision / sign on complex.

import math

c = 3 + 4j

# Presentation types
print(format(c, "f"))
print(format(c, "F"))
print(format(c, "e"))
print(format(c, "E"))
print(format(c, "g"))
print(format(c, "G"))
print(format(c, "n"))

# Precision
print(format(c, ".2f"))
print(format(c, ".3e"))
print(format(c, ".4g"))
print(format(c, ".0f"))

# Sign flags (apply to the real part; imaginary always carries an explicit sign)
print(format(c, "+.2f"))
print(format(c, " .2f"))
print(format(c, "-.2f"))

# Negative / mixed-sign components
print(format(complex(-1, -2), ".1f"))
print(format(complex(3, -4), ".2f"))
print(format(complex(-3, 4), ".2f"))
print(format(complex(-3, -4), "+.2f"))

# Zero / pure-imaginary components
print(format(complex(5, 0), ".2f"))
print(format(complex(0, 2), ".2f"))
print(format(0j, ".2f"))
print(format(complex(0, 4), "+.2f"))

# Negative zero
print(format(complex(-0.0, 0.0), ".2f"))
print(format(complex(0.0, -0.0), ".2f"))

# inf / nan components
print(format(complex(math.inf, math.nan), ".2f"))
print(format(complex(math.nan, math.inf), ".2f"))
print(format(complex(-math.inf, 4), ".1f"))
print(format(complex(math.nan, math.inf), "+f"))

# Width / fill / alignment apply to the whole assembled string
print(format(c, "20.2f"))
print(format(c, "<20.2f"))
print(format(c, ">20.2f"))
print(format(c, "^20.2f"))
print(format(c, "*<20.2f"))
print(format(c, "*^20.2f"))

# Grouping applies per component
print(format(complex(1234, 5), ",.2f"))
print(format(complex(1234567, 8), ","))

# f-string path matches format()
print(f"{c:.2f}")
print(f"{1 + 2j:.3f}")
print(f"{1 - 2j:.1f}")

# No-type spec: repr-style components, parenthesised unless real is +0
print(format(c, ""))
print(format(complex(0, 4), ""))
print(format(complex(3, 0), ""))
print(format(c, "20"))
print(format(c, "<20"))
print(format(c, "+"))
print(format(c, ".3"))
print(format(complex(3.14159, 2.71828), ".3"))
print(format(complex(0, 4), "+.3"))

# Invalid presentation types raise ValueError with CPython's message
for bad in ["d", "x", "o", "b", "c", "s", "%"]:
    try:
        format(c, bad)
        print("NO ERROR for " + bad)
    except ValueError as e:
        print(type(e).__name__ + ": " + str(e))

# Zero-padding and '=' alignment are rejected for complex
try:
    format(c, "020.2f")
except ValueError as e:
    print(type(e).__name__ + ": " + str(e))
try:
    format(c, "=20.2f")
except ValueError as e:
    print(type(e).__name__ + ": " + str(e))
