# Parity fixture for complex negative-zero display (issue #569).
# CPython 3.12 preserves the sign of -0.0 in both the real and imaginary
# parts; earlier code cast the component to i64, which silently dropped it.

# Real part is -0.0
print(complex(-0.0, 0))     # (-0+0j)
print(complex(-0.0, 1))     # (-0+1j)
print(complex(-0.0, -1))    # (-0-1j)
print(complex(-0.0, 1.5))   # (-0+1.5j)

# Imaginary part is -0.0
print(complex(0.0, -0.0))   # -0j
print(complex(1.5, -0.0))   # (1.5-0j)
print(complex(1, -0.0))     # (1-0j)

# Both parts are -0.0
print(complex(-0.0, -0.0))  # (-0-0j)

# Positive zero -- unaffected by the fix but included to guard regression
print(complex(0.0, 0.0))    # 0j
print(complex(1, 2))        # (1+2j)
print(complex(-1, -2))      # (-1-2j)

# -0.0 real with scientific-notation imaginary
print(complex(-0.0, 1e16))  # (-0+1e+16j)
print(complex(-0.0, 1e17))  # (-0+1e+17j)

# -0.0 imaginary with small float real
print(complex(1e-5, -0.0))  # (1e-05-0j)
print(complex(-0.0, 1e-5))  # (-0+1e-05j)
