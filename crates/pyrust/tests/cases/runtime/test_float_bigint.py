# Parity test: float() applied to BigInt values (numbers exceeding i64 range).
# float(bigint) must succeed for representable values and raise OverflowError
# for values outside f64's finite range — matching CPython 3.12 semantics.

x = 10**20
f = float(x)
print(type(f).__name__)       # float
print(f > 0)                  # True
print(f > 9e19)               # True  (1e+20 > 9e19)
print(f == 1e20)              # True
print(isinstance(f, float))   # True

# 2**64 exceeds i64::MAX; pyrust promotes it to BigInt
y = 2**64
g = float(y)
print(type(g).__name__)       # float
print(g > 0)                  # True

# Negative BigInt
z = -(10**20)
h = float(z)
print(type(h).__name__)       # float
print(h < 0)                  # True

# float(BigInt) == float(int) for values that fit in f64
a = 10**15
print(float(a) == float(1000000000000000))  # True

# BigInt too large for f64 => OverflowError (CPython parity)
try:
    float(10**400)
    print("no_error")
except OverflowError:
    print("overflow_ok")

# Negative BigInt too large for f64 => OverflowError
try:
    float(-(10**400))
    print("no_error")
except OverflowError:
    print("neg_overflow_ok")
