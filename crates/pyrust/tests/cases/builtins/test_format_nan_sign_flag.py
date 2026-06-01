# Parity fixture for issue #1938: float format must apply the '+' / ' ' sign
# flag to nan (and inf), matching CPython 3.12.
#
# CPython treats nan as unsigned in formatting: the sign bit is ignored, but an
# explicit '+' or ' ' flag still prepends a sign character.  pyrust's
# special-value branch previously dropped the flag for nan only.

nan = float("nan")
neg_nan = float("-nan")
inf = float("inf")
neg_inf = float("-inf")

# nan: '+' / ' ' flags apply across all presentation types
for spec in ["+", " ", "-", "+f", "+.2f", "+g", "+G", "+e", "+E", "+F", " e", "-f"]:
    print(repr(format(nan, spec)))

# negative-nan is still unsigned: the sign bit is ignored
for spec in ["+", " ", "-"]:
    print(repr(format(neg_nan, spec)))

# inf contrast: sign flag applies, real negative sign survives
for spec in ["+", " ", "-", "+f", "+e", "+g"]:
    print(repr(format(inf, spec)))
    print(repr(format(neg_inf, spec)))

# finite floats unchanged
for spec in ["+", " ", "-", "+.2f", "+e", "+g"]:
    print(repr(format(5.0, spec)))
    print(repr(format(-5.0, spec)))

# str.format() and f-strings route through the same path
print("{:+}".format(nan))
print("{: }".format(nan))
print(f"{nan:+}")
print(f"{inf:+}")

# %-style printf (separate code path) stays correct
print("%+f" % nan)
print("% f" % nan)
print("%+f" % inf)
