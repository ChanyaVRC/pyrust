# import, from-import, import-as, math module

import math
print("math-pi", math.pi)
print("math-e", math.e)
print("math-floor", math.floor(2.7))
print("math-ceil", math.ceil(2.3))
print("math-sqrt", math.sqrt(9.0))
print("math-fabs", math.fabs(-3.5))
print("math-pow", math.pow(2.0, 10.0))
print("math-log", math.log(1.0))
print("math-sin", math.sin(0.0))
print("math-cos", math.cos(0.0))
print("math-isnan", math.isnan(math.nan))
print("math-isinf", math.isinf(math.inf))

from math import pi, e, sqrt, floor
print("from-math-pi", pi)
print("from-math-e", e)
print("from-math-sqrt", sqrt(16.0))
print("from-math-floor", floor(3.9))

import math as m
print("import-as", m.pi)

# math.floor / math.ceil edge cases (Issue #97)
print("floor-normal", math.floor(1.9))
print("ceil-normal", math.ceil(-2.1))
print("floor-neg", math.floor(-1.3))
print("ceil-neg", math.ceil(-2.7))

# Large float → bignum (Issue #97)
big_floor = math.floor(1e100)
print("floor-bignum-large", big_floor > 2**62)
big_ceil = math.ceil(1e100)
print("ceil-bignum-large", big_ceil > 2**62)
