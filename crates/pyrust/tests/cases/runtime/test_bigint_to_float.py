# Parity fixture: BigInt arguments to math functions and format specs.
# Before the fix, any BigInt passed to try_value_to_float / value_to_float
# produced a spurious TypeError.  After the fix, finite-range BigInts convert
# correctly and out-of-range BigInts raise OverflowError (CPython 3.12 parity).

import math

# Finite-range BigInt -> correct float result
print(math.sqrt(10**20))           # 10000000000.0
print(round(math.log(2**100), 6))  # 69.314718
print(round(math.log2(2**100), 6)) # 100.0
print(math.floor(10**20))          # 100000000000000000000
print(math.ceil(-(10**20)))        # -100000000000000000000

# math.isinf: BigInt is never infinity
print(math.isinf(10**20))          # False

# format spec: BigInt formatted as float
n = 10**20
print(f'{n:.2f}')                   # 100000000000000000000.00

# Out-of-range BigInt -> OverflowError (not TypeError)
try:
    math.sqrt(2**10000)
except OverflowError as e:
    print(f'OverflowError: {e}')
