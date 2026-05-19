# Parity test: complex() with complex-valued arguments.
# CPython semantics: complex(real, imag) when at least one arg is complex:
#   cr, ci = (real.real, real.imag) if isinstance(real, complex) else (float(real), 0.0)
#   dr, di = (imag.real, imag.imag) if isinstance(imag, complex) else (float(imag), 0.0)
#   result = complex(cr - di, ci + dr)
# When neither arg is complex, real and imag are assigned directly (no formula).

# Two-arg form with complex first arg
print(complex(1+2j, 3))       # (1+5j)
print(complex(1+2j, 3j))      # (-2+2j)
print(complex(1+2j, 1+2j))    # (-1+3j)

# Two-arg form with complex second arg
print(complex(0, 1+2j))       # (-2+1j)
print(complex(3, 2j))         # (1+0j)

# Both imaginary-only complexes
print(complex(1j, 2j))        # (-2+1j)

# Zero complex args
print(complex(0+0j, 0))       # 0j
print(complex(0, 0+0j))       # 0j

# Regressions: scalar args still work (direct assignment, no formula)
print(complex(1, 2))           # (1+2j)
print(complex(1.5, 2.5))       # (1.5+2.5j)
print(complex(True, False))    # (1+0j)

# One-arg passthrough
print(complex(1+2j))           # (1+2j)
print(complex(0))              # 0j

# One-arg string
print(complex("1+2j"))         # (1+2j)

# Two-arg errors
try:
    complex("1+2j", 3)
except TypeError as e:
    print(type(e).__name__, e)

try:
    complex(1, "3")
except TypeError as e:
    print(type(e).__name__, e)
