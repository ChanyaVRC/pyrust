# Non-finite floats (inf/nan) with zero-fill + grouping must NOT group the
# synthetic zero-padding digits: CPython emits a solid fill block before the
# inf/nan token (#2504).
inf = float("inf")
ninf = float("-inf")
nan = float("nan")

# Zero-fill + comma grouping.
print(repr(f"{inf:+015,.3f}"))
print(repr(f"{ninf:+015,.3f}"))
print(repr(f"{nan:+015,.3f}"))
print(repr(f"{inf:015,}"))
print(repr(f"{nan:015,}"))

# Underscore grouping is treated the same way.
print(repr(f"{inf:015_.3f}"))
print(repr(f"{nan:015_}"))

# Uppercase, exponential, and percent presentation types with grouping.
print(repr(f"{inf:+020,.2E}"))
print(repr(f"{ninf:020,F}"))
print(repr(f"{nan:+015,%}"))

# Sign-space and explicit left-align combined with grouping zero-fill.
print(repr(f"{inf: 015,.1f}"))
print(repr(f"{inf:<015,.1f}"))

# Finite zero-fill + grouping must be unaffected.
print(repr(f"{1234.5:015,.2f}"))
print(repr(f"{0.001:015,.6f}"))
