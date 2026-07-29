import math

# Negative real ** non-integer float → complex (principal log branch)
r = (-1) ** 0.5
print(abs(r.imag - 1.0) < 1e-9 and abs(r.real) < 1e-9)    # True

r = (-1.0) ** 0.5
print(abs(r.imag - 1.0) < 1e-9 and abs(r.real) < 1e-9)    # True

r = (-4) ** 0.5
print(abs(r.imag - 2.0) < 1e-9 and abs(r.real) < 1e-9)    # True

r = (-8) ** (1/3)
# |(-8)^(1/3)| = 2; arg = pi/3 → real=1, imag=sqrt(3)
print(abs(r.real - 1.0) < 1e-9 and abs(r.imag - math.sqrt(3)) < 1e-9)   # True

# Negative exponent: (-2) ** -0.5 → complex
r = (-2) ** -0.5
# |(-2)^(-0.5)| = 1/sqrt(2); arg = -pi/2 → real≈0, imag=-1/sqrt(2)
print(abs(r.real) < 1e-9 and abs(r.imag - (-1.0 / math.sqrt(2))) < 1e-9)   # True

# Large negative int base (BigInt path) → complex
r = (-1000000000000) ** 0.5
print(abs(r.imag - 1000000.0) < 1e-3 and abs(r.real) < 1e-3)   # True

# These should remain real (no complex promotion)
print((-1) ** 2)       # 1
print((-2) ** 3)       # -8
print((-1.0) ** 2.0)   # 1.0
print((-1) ** 2.0)     # 1.0
print(4 ** 0.5)        # 2.0
print(4.0 ** 0.5)      # 2.0
