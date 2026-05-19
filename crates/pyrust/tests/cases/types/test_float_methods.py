# Tests for float instance methods: is_integer, as_integer_ratio, hex
# and the class method fromhex.  All output must match CPython 3.12.

# --- is_integer ---
print((1.0).is_integer())        # True
print((1.5).is_integer())        # False
print((0.0).is_integer())        # True
print(float('inf').is_integer()) # False  (infinity is NOT an integer)
print(float('nan').is_integer()) # False

# --- as_integer_ratio ---
print((1.5).as_integer_ratio())  # (3, 2)
print((0.5).as_integer_ratio())  # (1, 2)
print((1.0).as_integer_ratio())  # (1, 1)
print((-0.5).as_integer_ratio()) # (-1, 2)
print((0.0).as_integer_ratio())  # (0, 1)
print((-0.0).as_integer_ratio()) # (0, 1)  sign of -0.0 is ignored

# Large float: numerator > i64::MAX, must use BigInt
# (2**53).as_integer_ratio() == (9007199254740992, 1)
print(((2**53)*1.0).as_integer_ratio())

# as_integer_ratio error cases
try:
    float('inf').as_integer_ratio()
except OverflowError as e:
    print(e)
try:
    float('nan').as_integer_ratio()
except ValueError as e:
    print(e)

# --- hex ---
print((1.0).hex())    # 0x1.0000000000000p+0
print((1.5).hex())    # 0x1.8000000000000p+0
print((-1.0).hex())   # -0x1.0000000000000p+0
print((0.0).hex())    # 0x0.0p+0
print((-0.0).hex())   # -0x0.0p+0
print(float('inf').hex())   # inf
print(float('-inf').hex())  # -inf
print(float('nan').hex())   # nan

# Subnormal: exponent always displayed as -1022
print((5e-324).hex())  # 0x0.0000000000001p-1022

# --- fromhex ---
print(float.fromhex('0x1.0p+0'))    # 1.0
print(float.fromhex('0x1.8p+0'))    # 1.5
print(float.fromhex('-0x1.0p+0'))   # -1.0
print(float.fromhex('0x0.0p+0'))    # 0.0
print(float.fromhex('inf'))         # inf
print(float.fromhex('-inf'))        # -inf
print(float.fromhex('nan'))         # nan

# fromhex without 0x prefix is allowed
print(float.fromhex('1.0p+0'))      # 1.0

# Leading/trailing whitespace is stripped
print(float.fromhex('  0x1.0p+0  '))  # 1.0

# Round-trip: hex -> fromhex -> hex
for x in [1.0, 1.5, -1.0, 1e100, 1e-100]:
    h = x.hex()
    y = float.fromhex(h)
    print(x == y)  # True

# fromhex error
try:
    float.fromhex('not-a-float')
except ValueError as e:
    print(e)
