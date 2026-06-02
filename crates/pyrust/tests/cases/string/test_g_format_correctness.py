# Parity fixture for issues #1950 and #2000: 'g'/'G' (and 'n') float formatting.
#
# #1950: the alternate '#' form must KEEP trailing zeros out to P significant
#        figures (and keep the decimal point) in BOTH str.format and %-printf.
# #2000: the value is rounded to P significant digits FIRST, then the
#        fixed-vs-exponent decision is made from the ROUNDED exponent, so a
#        value that rounds up across a power of ten (e.g. 999999.5 -> 1e+06)
#        flips into exponential notation.

# ── #1950: alternate '#g'/'#G' keeps trailing zeros (str.format / format) ─────
print(repr("{:#g}".format(1.5)))      # '1.50000'
print(repr("{:#g}".format(2.5)))      # '2.50000'
print(repr("{:#g}".format(0.5)))      # '0.500000'
print(repr("{:#g}".format(1.0)))      # '1.00000'
print(repr("{:#g}".format(100.0)))    # '100.000'
print(repr(format(0.0, "#g")))        # '0.00000'
print(repr(format(-3.25, "#g")))      # '-3.25000'
print(repr(format(123.456, "#.3g")))  # '123.'
print(repr(format(0.000123456, "#.4g")))  # '0.0001235' (negative exp fixed)
print(repr(format(9.99e-7, "#g")))    # exponential, zeros kept

# Uppercase #G
print(repr("{:#G}".format(1.5)))      # '1.50000'
print(repr(format(1.0e20, "#G")))     # '1.00000E+20'

# ── #1950: alternate '#g' keeps trailing zeros (%-printf) ────────────────────
print(repr("%#g" % 1.0))      # '1.00000'
print(repr("%#.3g" % 1.0))    # '1.00'
print(repr("%#.6g" % 1.0))    # '1.00000'
print(repr("%#.1g" % 1.0))    # '1.'
print(repr("%#g" % 1.5))      # '1.50000'
print(repr("%#.10g" % 0.5))   # '0.5000000000'
print(repr("%#G" % 12345.0))  # '12345.0'

# ── #2000: rounding decides fixed-vs-exponent ────────────────────────────────
print(repr(format(999999.5, "g")))        # '1e+06'
print(repr(format(99999.99999, ".5g")))   # '1e+05'
print(repr(format(9.9999, ".4g")))        # '10'
print(repr(format(9.99999e3, ".5g")))     # '10000'
print(repr(format(9.99999e3, ".4g")))     # '1e+04'
print(repr("%g" % 999999.5))              # '1e+06'
print(repr("%.5g" % 99999.99999))         # '1e+05'

# rounding-crosses-exponent sweep across powers of ten and precisions
for k in range(-4, 6):
    for mant in [9.9999, 9.99999, 9.9]:
        v = mant * (10.0 ** k)
        for spec in ["g", "G", ".3g", ".5g", ".2g"]:
            print(spec, repr(v), repr(format(v, spec)))

# ── non-'#' default 'g' still strips trailing zeros (unchanged) ──────────────
print(repr(format(1.5, "g")))     # '1.5'
print(repr(format(1.0, "g")))     # '1'
print(repr(format(100.0, "g")))   # '100'
print(repr(format(0.0, "g")))     # '0'

# ── n is identical to g in C-locale ──────────────────────────────────────────
for v in [1.5, 1.0, 100.0, 999999.5, 0.000123456, 9.99999e3]:
    assert format(v, "n") == format(v, "g"), (v, format(v, "n"), format(v, "g"))
    assert format(v, ".5n") == format(v, ".5g")
print("n==g ok")
