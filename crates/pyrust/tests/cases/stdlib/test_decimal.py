from decimal import Decimal, getcontext, setcontext, localcontext, ROUND_HALF_UP
import decimal

# Basic arithmetic
print(Decimal('1.5') + Decimal('2.5'))          # 4.0
print(Decimal('1') / Decimal('3'))              # 28-digit result
print(Decimal('2') ** 3)                        # 8
print(Decimal('1.5') * Decimal('2'))            # 3.0
print(abs(Decimal('-1.5')))                     # 1.5
print(Decimal('17') % Decimal('5'))             # 2
print(divmod(Decimal('17'), Decimal('5')))      # (Decimal('3'), Decimal('2'))
print(Decimal('-7') // Decimal('2'))            # -3
print(Decimal('10') ** -2)                      # 0.01

# Special values
print(Decimal('Inf'))                           # Infinity
print(Decimal('-Inf'))                          # -Infinity
print(Decimal('NaN'))                           # NaN
print(Decimal('Infinity'))                      # Infinity
print(Decimal('Inf') + Decimal('1'))            # Infinity
print(Decimal('NaN') + Decimal('1'))            # NaN

# Context
print(getcontext().prec)                        # 28

# from_float (exact decimal expansion of the binary float)
print(Decimal.from_float(0.25))                 # 0.25
print(Decimal.from_float(0.1))                  # 0.10000000000000000555...
print(Decimal.from_float(2.5))                  # 2.5
print(Decimal.from_float(-0.0))                 # -0
print(Decimal.from_float(100))                  # 100

# Comparisons (Decimal vs Decimal, int, float)
print(Decimal('1.5') > Decimal('1.0'))          # True
print(Decimal('1.5') == Decimal('1.50'))        # True
print(Decimal('0') == 0)                         # True
print(Decimal('5') == 5)                         # True
print(Decimal('1.5') == 1.5)                     # True

# String representation preserves trailing zeros
print(str(Decimal('1.0')))                      # 1.0
print(str(Decimal('1.50')))                     # 1.50

# repr form
print(repr(Decimal('1.5')))                     # Decimal('1.5')
print(repr(Decimal('NaN')))                     # Decimal('NaN')

# int / float / bool conversion
print(int(Decimal('3.7')))                      # 3
print(int(Decimal('-3.9')))                     # -3
print(float(Decimal('1.5')))                    # 1.5
print(bool(Decimal('0')), bool(Decimal('1')))   # False True

# Rounding
print(Decimal('2.5').quantize(Decimal('1'), rounding=ROUND_HALF_UP))  # 3
print(Decimal('1.5').quantize(Decimal('0.1')))                        # 1.5
print(Decimal('3.14159').quantize(Decimal('0.01')))                   # 3.14
print(round(Decimal('1.5')))                    # 2 (banker's rounding)
print(round(Decimal('2.5')))                    # 2
print(round(Decimal('3.14159'), 2))             # 3.14

# Zero
print(Decimal(0))                               # 0
print(Decimal('0') == 0)                         # True

# Context manager
with localcontext() as ctx:
    ctx.prec = 5
    print(Decimal('1') / Decimal('3'))          # 0.33333
print(Decimal('1') / Decimal('3'))              # back to 28 digits

# Misc methods
print(Decimal('100').sqrt())                    # 10
print(Decimal('1.30').normalize())              # 1.3
print(Decimal('-1.5').copy_abs())               # 1.5
print(max(Decimal('1'), Decimal('2'), Decimal('1.5')))   # 2
print(sorted([Decimal('3'), Decimal('1'), Decimal('2')]))  # [1, 2, 3]
print(hash(Decimal('1.5')) == hash(Decimal('1.50')))     # True

# Exception type / hierarchy (message text differs between CPython's C and
# pure-Python implementations, so only the type is asserted here).
try:
    Decimal('Inf') - Decimal('Inf')
except decimal.InvalidOperation as e:
    print("InvalidOperation:", isinstance(e, decimal.DecimalException),
          isinstance(e, ArithmeticError))

print("decimal ok")
