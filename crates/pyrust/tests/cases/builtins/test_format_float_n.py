# Parity fixture for issue #1948: the 'n' presentation type must work on
# floats, not just ints.  `format(3.14, "n")` previously raised
# "Unknown format code 'n' for object of type 'float'".
#
# 'n' is locale-aware; in the C/POSIX locale (CPython's default and what the
# parity harness runs under) it is identical to 'g' for floats: same default
# precision, same trailing-zero stripping, same exponent threshold, lowercase.

# Bare 'n' across a wide value range, including the exponent-threshold cases.
for v in [3.14, 1234.5, 1e20, 1e16, 1e-5, 1234567.0, 0.1, 0.0, -0.0,
          1.0, -1.0, -3.14, 123456789.0, 0.000123, 9999999.0, 100000.0,
          1e-300, 1e300, -2.5e-10]:
    print(repr(format(v, "n")))

# Precision variants on floats: '.<prec>n' == '.<prec>g'.
for v in [3.14159, 1234.5, 1234567.0, 0.000123]:
    for spec in [".0n", ".1n", ".2n", ".5n", ".10n", ".3n"]:
        print(repr(format(v, spec)))

# inf / nan: 'n' ignores precision and renders lowercase, but honours the
# explicit sign flag.
inf = float("inf")
nan = float("nan")
for spec in ["n", "+n", " n", ".2n", "10n"]:
    print(repr(format(inf, spec)))
    print(repr(format(-inf, spec)))
    print(repr(format(nan, spec)))

# Width / sign / zero-pad combine with 'n' the same way they do with 'g'.
for spec in ["10n", "+n", " n", "020.3n", "<10n", ">10n", "^10n"]:
    print(repr(format(1234.5, spec)))
    print(repr(format(-1234.5, spec)))

# For floats, 'n' must equal 'g' byte-for-byte (the relationship the fix relies
# on).  Assert it directly so a future divergence is caught here.
for v in [3.14, 1234.5, 1e20, 1234567.0, 0.000123, 1e-5, 100000.0]:
    for spec in ["n", ".0n", ".2n", ".10n", "+n", "10n"]:
        assert format(v, spec) == format(v, spec.replace("n", "g")), (v, spec)
print("n == g for floats: OK")

# int / bool 'n' is unchanged (routed to the integer formatter).
for v in [1000000, 0, -42, True, False, 255]:
    print(repr(format(v, "n")))

# str.format() and f-strings route through the same path.
print("{:n}".format(2.5))
print(f"{2.5:n}")
print(f"{1234567.0:.2n}")

# complex 'n' (already supported before the fix; guard against regression).
print(repr(format(1 + 2j, "n")))
