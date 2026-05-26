# int(), float(), complex() dunder dispatch — CPython 3.12 parity.
# Regression guard: PR #1155 accidentally deleted PyInstance arms from all three.

# int() with __int__
class MyInt:
    def __int__(self): return 42
print(int(MyInt()))

# int() with __index__ (no __int__)
class MyIndex:
    def __index__(self): return 99
print(int(MyIndex()))

# int() with unknown type raises TypeError
class NoConv:
    pass
try:
    int(NoConv())
except TypeError as e:
    print(e)

# float() with __float__
class MyFloat:
    def __float__(self): return 3.14
print(float(MyFloat()))

# float() with __index__ (no __float__)
class MyIndexF:
    def __index__(self): return 5
print(float(MyIndexF()))

# float() with unknown type raises TypeError
try:
    float(NoConv())
except TypeError as e:
    print(e)

# complex() with __complex__
class MyComplex:
    def __complex__(self): return 1+2j
print(complex(MyComplex()))

# complex() with __float__ (no __complex__)
class MyFloatC:
    def __float__(self): return 2.5
print(complex(MyFloatC()))

# complex() with unknown type raises TypeError
try:
    complex(NoConv())
except TypeError as e:
    print(e)
