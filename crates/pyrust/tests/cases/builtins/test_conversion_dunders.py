# Parity fixture: int(), float(), complex() dispatch __int__, __float__,
# __complex__, and __index__ on user-defined objects.
# CPython reference: 3.12+ (Objects/abstract.c, Objects/complexobject.c).

# --- int() dispatch ---

class MyInt:
    def __int__(self): return 42

class MyIndex:
    def __index__(self): return 7

class BothIntAndIndex:
    # __int__ takes priority over __index__
    def __int__(self): return 10
    def __index__(self): return 20

class BigIntReturn:
    def __int__(self): return 2**100

class BadInt:
    def __int__(self): return "42"

print(int(MyInt()))            # 42
print(int(MyIndex()))          # 7
print(int(BothIntAndIndex()))  # 10 (__int__ wins)
print(int(BigIntReturn()))     # 1267650600228229401496703205376

try:
    int(BadInt())
except TypeError as e:
    print("TypeError:", e)

# No-dunder object raises TypeError
class NoConvInt:
    pass

try:
    int(NoConvInt())
except TypeError as e:
    print("TypeError:", e)

# Regression: primitives still work
print(int(3.7))    # 3
print(int("42"))   # 42
print(int(True))   # 1

# --- float() dispatch ---

class MyFloat:
    def __float__(self): return 3.14

class FloatFromIndex:
    def __index__(self): return 7

class BothFloatAndIndex:
    # __float__ takes priority
    def __float__(self): return 1.5
    def __index__(self): return 20

class BadFloat:
    def __float__(self): return "3.14"

print(float(MyFloat()))            # 3.14
print(float(FloatFromIndex()))     # 7.0
print(float(BothFloatAndIndex()))  # 1.5

try:
    float(BadFloat())
except TypeError as e:
    print("TypeError:", e)

class NoConvFloat:
    pass

try:
    float(NoConvFloat())
except TypeError as e:
    print("TypeError:", e)

# Regression
print(float(3))    # 3.0
print(float(True)) # 1.0

# --- complex() dispatch (1-arg) ---

class MyComplex:
    def __complex__(self): return complex(1, 2)

class ComplexFromFloat:
    def __float__(self): return 2.5

class ComplexFromIndex:
    def __index__(self): return 5

class ComplexPriority:
    # __complex__ takes priority over __float__
    def __complex__(self): return complex(3, 4)
    def __float__(self): return 99.0

class BadComplex:
    def __complex__(self): return "1+2j"

print(complex(MyComplex()))       # (1+2j)
print(complex(ComplexFromFloat())) # (2.5+0j)
print(complex(ComplexFromIndex())) # (5+0j)
print(complex(ComplexPriority()))  # (3+4j)

try:
    complex(BadComplex())
except TypeError as e:
    print("TypeError:", e)

class NoConvComplex:
    pass

try:
    complex(NoConvComplex())
except TypeError as e:
    print("TypeError:", e)

# Regression
print(complex(1, 2))    # (1+2j)
print(complex(1.5, 0))  # (1.5+0j)

# --- complex() dispatch (2-arg) ---

class ArgFloat:
    def __float__(self): return 2.5

class ArgIndex:
    def __index__(self): return 3

class FirstComplex:
    def __complex__(self): return complex(3, 4)

# First arg with __float__
print(complex(ArgFloat(), 1))  # (2.5+1j)
print(complex(1, ArgFloat()))  # (1+2.5j)

# First arg with __index__
print(complex(ArgIndex(), 1))  # (3+1j)
print(complex(1, ArgIndex()))  # (1+3j)

# First arg with __complex__ (yields complex; applies decomposition formula)
print(complex(FirstComplex(), 2))  # (3+6j): cr=3, ci=4, dr=2, di=0 → (3-0, 4+2)

# Second arg only has __complex__ → TypeError (CPython doesn't call __complex__ for second)
try:
    complex(1, FirstComplex())
except TypeError as e:
    print("TypeError:", e)

# 2-arg: non-PyInstance non-primitive errors
try:
    complex([1], 1)
except TypeError as e:
    print("TypeError:", e)

try:
    complex(1, [1])
except TypeError as e:
    print("TypeError:", e)
