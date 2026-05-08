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
