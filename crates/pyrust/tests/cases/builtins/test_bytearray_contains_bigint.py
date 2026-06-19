ba = bytearray(b'abc')

# BigInt -> ValueError
try:
    _ = (1 << 70) in ba
    print('WRONG')
except ValueError as e:
    print('ok', str(e) == "byte must be in range(0, 256)")

# Negative BigInt -> ValueError
try:
    _ = (-(1 << 70)) in ba
    print('WRONG')
except ValueError as e:
    print('ok')

# Sanity: in-range values still work
print(97 in ba)     # True  (ord('a'))
print(128 in ba)    # False
print(0 in ba)      # False

# Out-of-range plain int still ValueError (regression check)
try:
    _ = 256 in ba
    print('WRONG')
except ValueError:
    print('ok 256')
