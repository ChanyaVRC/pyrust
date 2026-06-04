# format() '%' presentation type appends '%' for inf / nan (#2027).
#
# The inf/nan early-return branch in float formatting skipped the per-type
# suffix, so "{:%}".format(inf) returned 'inf' instead of CPython's 'inf%'.
inf = float('inf')
nan = float('nan')

for v in [inf, -inf, nan]:
    print(repr("{:%}".format(v)))
    print(repr("{:.1%}".format(v)))
    print(repr("{:.2%}".format(v)))
    print(repr("{:+%}".format(v)))     # explicit sign flag still applies
    print(repr("{:>8%}".format(v)))    # width / align still apply

print(repr(format(nan, '%')))
print(repr(format(inf, '%')))

# Finite percent formatting is unchanged.
print(repr("{:%}".format(0.5)))
print(repr("{:.2%}".format(0.5)))
