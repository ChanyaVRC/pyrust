# Test that complex() accepts BigInt arguments, matching CPython 3.12.
# BigInt values (e.g. 10**20) must be converted to float, not rejected with TypeError.

# One-arg form: BigInt real part
print(complex(10**20))          # (1e+20+0j)
print(complex(2**100))          # (1.2676506002282294e+30+0j)
print(complex(2**53))           # (9007199254740992+0j)

# Two-arg form: BigInt real
print(complex(10**20, 0))       # (1e+20+0j)
print(complex(10**20, 1.5))     # (1e+20+1.5j)

# Two-arg form: BigInt imag
print(complex(0, 10**20))       # 1e+20j
print(complex(1, 2**100))       # (1+1.2676506002282294e+30j)
print(complex(1, 2**53))        # (1+9007199254740992j)
print(complex(1.5, 10**20))     # (1.5+1e+20j)

# Two-arg form: both BigInt
print(complex(10**20, 10**20))  # (1e+20+1e+20j)

# Overflow: BigInt too large for f64 -> OverflowError (not TypeError)
try:
    complex(10**400)
except OverflowError:
    print("OverflowError: real overflow")

try:
    complex(0, 10**400)
except OverflowError:
    print("OverflowError: imag overflow")

try:
    complex(10**400, 10**400)
except OverflowError:
    print("OverflowError: both overflow")

# Mixed bool + BigInt
print(complex(True, 10**20))    # (1+1e+20j)
print(complex(10**20, True))    # (1e+20+1j)

# Regression: existing int/float/bool/complex args still work
print(complex(1, 2))            # (1+2j)
print(complex(1.5, 2.5))       # (1.5+2.5j)
print(complex(True, False))     # (1+0j)
print(complex())                # 0j
print(complex(5))               # (5+0j)
