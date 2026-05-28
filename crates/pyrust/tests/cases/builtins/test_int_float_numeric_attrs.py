# Parity fixture for int/float numeric-tower attributes (issue #1341).
# CPython reference: 3.12+
# int.real, int.imag, int.conjugate(), int.numerator, int.denominator
# float.real, float.imag, float.conjugate()

# --- int properties ---
print((42).real)          # 42
print((42).imag)          # 0
print((42).conjugate())   # 42
print((42).numerator)     # 42
print((42).denominator)   # 1

print((-5).real)          # -5
print((-5).imag)          # 0
print((-5).conjugate())   # -5
print((-5).numerator)     # -5
print((-5).denominator)   # 1

print((0).real)           # 0
print((0).imag)           # 0
print((0).conjugate())    # 0
print((0).numerator)      # 0
print((0).denominator)    # 1

# --- BigInt ---
big = 10 ** 50
print(big.real == big)    # True
print(big.imag)           # 0
print(big.denominator)    # 1
print(big.conjugate() == big)  # True

# --- bool inherits from int; real/conjugate/numerator return int, not bool ---
print(True.real)          # 1
print(type(True.real).__name__)  # int
print(True.imag)          # 0
print(True.conjugate())   # 1
print(type(True.conjugate()).__name__)  # int
print(True.numerator)     # 1
print(type(True.numerator).__name__)  # int
print(True.denominator)   # 1

print(False.real)         # 0
print(False.imag)         # 0
print(False.conjugate())  # 0
print(False.numerator)    # 0
print(False.denominator)  # 1

# --- float properties (no numerator/denominator) ---
print((3.14).real)        # 3.14
print((3.14).imag)        # 0.0
print((3.14).conjugate()) # 3.14

print((0.0).real)         # 0.0
print((0.0).imag)         # 0.0
print((0.0).conjugate())  # 0.0

print((-2.5).real)        # -2.5
print((-2.5).imag)        # 0.0
print((-2.5).conjugate()) # -2.5

# float has no numerator/denominator
try:
    print((3.14).numerator)
except AttributeError as e:
    print(e)

try:
    print((3.14).denominator)
except AttributeError as e:
    print(e)

# conjugate() takes no arguments
try:
    (42).conjugate(1)
except TypeError as e:
    print(e)

try:
    (3.14).conjugate(1)
except TypeError as e:
    print(e)

# --- int and float subclasses inherit numeric-tower properties ---
class MyInt(int):
    pass

mi = MyInt(42)
print(mi.real)        # 42
print(mi.imag)        # 0
print(mi.numerator)   # 42
print(mi.denominator) # 1

class MyFloat(float):
    pass

mf = MyFloat(3.14)
print(mf.real)        # 3.14
print(mf.imag)        # 0.0
print(mf.conjugate()) # 3.14
