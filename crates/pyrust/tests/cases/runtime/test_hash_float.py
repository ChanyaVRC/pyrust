print(hash(0.5))    # 1152921504606846976
print(hash(1.5))    # 1152921504606846977
print(hash(-0.5))   # -1152921504606846976
print(hash(0.1))    # 230584300921369408
print(hash(0.0))    # 0
print(hash(1.0))    # 1  (integer-valued floats hash same as int)
print(hash(2.0))    # 2
print(hash(-1.0))   # -2  (sentinel remap: -1 is reserved)
print(hash(-2.0))   # -2  (negative integer-valued float)
print(hash(float('inf')))    # 314159  (sys.hash_info.inf)
print(hash(float('-inf')))   # -314159
# hash(float('nan')) is excluded: CPython uses object-identity (id//16),
# which is process-local and not stable across runs.

# Integer-float consistency (CPython guarantee: hash(n) == hash(float(n)))
print(hash(1) == hash(1.0))   # True
print(hash(2) == hash(2.0))   # True
print(hash(-1) == hash(-1.0)) # True  (both are -2)
print(hash(0) == hash(0.0))   # True  (both are 0)

# Subnormal float
print(hash(5e-324))  # 16777216

# Larger values
print(hash(0.25))    # 576460752303423488
print(hash(0.125))   # 288230376151711744
