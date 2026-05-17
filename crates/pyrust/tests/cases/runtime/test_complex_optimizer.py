# Complex literals survive optimization passes and constant-pool deduplication.
# Exercises is_pure_expr, const_eq, and intern_const_in_pool for Complex values.

def f():
    return 1+2j

print(f())           # (1+2j)
print(type(f()).__name__)  # complex

# Module-level complex constant
z = 3.14j
print(z)             # 3.14j
print(z.real)        # 0.0
print(z.imag)        # 3.14

# Repeated calls — verifies that the constant pool stays stable across calls
# (each call returns the same value, not None or a wrong value).
print(f())           # (1+2j)
print(f())           # (1+2j)

# Pure function with complex literal — used as a loop-body constant.
# The constant should not be incorrectly dropped by the optimizer.
def g():
    total = 0j
    for _ in range(3):
        total = total + (0+1j)
    return total

print(g())           # 3j

# Imaginary-only literal
print(1j)            # 1j
print(1j.real)       # 0.0
print(1j.imag)       # 1.0

# Real-only complex
c = complex(5, 0)
print(c.real)        # 5.0
print(c.imag)        # 0.0
