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

# CPython 3.10+ hashes NaN by object identity.  Do not compare the process-local
# numbers directly; retain every object simultaneously and check the observable
# invariants instead.
float_nans = [float("nan") for _ in range(16)]
float_nan_hashes = [hash(value) for value in float_nans]
print(len(set(float_nan_hashes)) == len(float_nans))
print(all(value != 0 for value in float_nan_hashes))
print(all(hash(value) == expected for value, expected in zip(float_nans, float_nan_hashes)))

# complex.__hash__ delegates each component to the float hash algorithm, so a
# NaN-bearing complex has the same per-object requirements.
complex_nans = [complex(float("nan"), 1.0) for _ in range(16)]
complex_nan_hashes = [hash(value) for value in complex_nans]
print(len(set(complex_nan_hashes)) == len(complex_nans))
print(all(value != 0 for value in complex_nan_hashes))
print(all(hash(value) == expected for value, expected in zip(complex_nans, complex_nan_hashes)))

# Decimal NaNs already use object.__hash__; keep that separate path pinned.
from decimal import Decimal
decimal_nans = [Decimal("NaN") for _ in range(16)]
decimal_nan_hashes = [hash(value) for value in decimal_nans]
print(len(set(decimal_nan_hashes)) == len(decimal_nans))
print(all(value != 0 for value in decimal_nan_hashes))
print(all(hash(value) == expected for value, expected in zip(decimal_nans, decimal_nan_hashes)))

# NaN keys stay distinct, and rebuilding the values stored by a container must
# preserve both identity and hash.
float_nan_dict = {value: index for index, value in enumerate(float_nans)}
float_nan_set = set(float_nans)
round_tripped = list(float_nan_dict)
print(len(float_nan_dict) == len(float_nans), len(float_nan_set) == len(float_nans))
print(all(original is restored for original, restored in zip(float_nans, round_tripped)))
print(all(hash(original) == hash(restored) for original, restored in zip(float_nans, round_tripped)))

# Integer-float consistency (CPython guarantee: hash(n) == hash(float(n)))
print(hash(1) == hash(1.0))   # True
print(hash(2) == hash(2.0))   # True
print(hash(-1) == hash(-1.0)) # True  (both are -2)
print(hash(0) == hash(0.0))   # True  (both are 0)
print(hash(1) == hash(1.0) == hash(1 + 0j))
print(hash(-1), hash(-1.0), hash(complex(-1.0, 0.0)))

# Subnormal float
print(hash(5e-324))  # 16777216

# Larger values
print(hash(0.25))    # 576460752303423488
print(hash(0.125))   # 288230376151711744
