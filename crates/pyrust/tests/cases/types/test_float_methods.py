# CPython 3.12 parity: float instance and class methods

# --- is_integer ---
print((1.0).is_integer())   # True
print((1.5).is_integer())   # False
print((0.0).is_integer())   # True
print(float('nan').is_integer())  # False
print(float('inf').is_integer())  # False
print((-2.0).is_integer())  # True

# --- as_integer_ratio ---
print((1.5).as_integer_ratio())   # (3, 2)
print((0.5).as_integer_ratio())   # (1, 2)
print((1.0).as_integer_ratio())   # (1, 1)
print((2.0).as_integer_ratio())   # (2, 1)
print((-1.5).as_integer_ratio())  # (-3, 2)

try:
    float('inf').as_integer_ratio()
except OverflowError:
    print("OverflowError")  # OverflowError

try:
    float('nan').as_integer_ratio()
except ValueError:
    print("ValueError")  # ValueError

# --- hex ---
print((1.0).hex())    # 0x1.0000000000000p+0
print((1.5).hex())    # 0x1.8000000000000p+0
print((-0.0).hex())   # -0x0.0p+0
print((2.0).hex())    # 0x1.0000000000000p+1
print(float('inf').hex())   # inf
print(float('nan').hex())   # nan

# --- fromhex ---
print(float.fromhex('0x1.0p+0'))        # 1.0
print(float.fromhex('0x1.8p+0'))        # 1.5
print(float.fromhex('  0x1.0p+0  '))   # 1.0 (whitespace OK)
print(float.fromhex('-0x1.0p+0'))       # -1.0
print(float.fromhex('inf'))             # inf
print(float.fromhex('-inf'))            # -inf
print(float.fromhex('1.0p0'))          # 1.0 (no 0x prefix OK)

try:
    float.fromhex('not_a_float')
except ValueError:
    print("ValueError")  # ValueError

try:
    float.fromhex('0x1.0p+1100')
except OverflowError:
    print("OverflowError")  # OverflowError

# --- hasattr ---
print(hasattr(1.0, 'is_integer'))       # True
print(hasattr(1.0, 'as_integer_ratio')) # True
print(hasattr(1.0, 'hex'))             # True
print(hasattr(1.0, 'fromhex'))         # True
print(hasattr(float, 'fromhex'))       # True
