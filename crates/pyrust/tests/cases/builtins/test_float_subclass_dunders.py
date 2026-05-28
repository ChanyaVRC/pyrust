"""
Parity fixture: math.trunc/floor/ceil on float subclass instances dispatch
__trunc__/__floor__/__ceil__ via MRO rather than raising TypeError.

Issue #1452: float.__trunc__, float.__floor__, float.__ceil__ were not
registered on the built-in float PyClass, so subclass instances could not
resolve them via MRO lookup.
"""
import math


class MyFloat(float):
    pass


# Basic numeric cases — plain subclass inherits float.__trunc__/floor/ceil.
print(math.trunc(MyFloat(1.9)))    # 1
print(math.floor(MyFloat(1.9)))    # 1
print(math.ceil(MyFloat(1.1)))     # 2
print(math.trunc(MyFloat(-1.9)))   # -1
print(math.floor(MyFloat(-1.9)))   # -2
print(math.ceil(MyFloat(-1.1)))    # -1

# Custom override — user-defined dunders take precedence over inherited ones.
class MyFloat2(float):
    def __trunc__(self):
        return 42

    def __floor__(self):
        return 99

    def __ceil__(self):
        return 77


print(math.trunc(MyFloat2(1.9)))   # 42
print(math.floor(MyFloat2(1.9)))   # 99
print(math.ceil(MyFloat2(1.1)))    # 77

# Plain float still works — regression guard.
print(math.trunc(1.9))   # 1
print(math.floor(1.9))   # 1
print(math.ceil(1.1))    # 2

# infinity → OverflowError (not TypeError).
try:
    math.trunc(MyFloat(float('inf')))
except OverflowError as e:
    print("OverflowError:", e)

try:
    math.floor(MyFloat(float('inf')))
except OverflowError as e:
    print("OverflowError:", e)

try:
    math.ceil(MyFloat(float('inf')))
except OverflowError as e:
    print("OverflowError:", e)

# NaN → ValueError (not TypeError).
try:
    math.trunc(MyFloat(float('nan')))
except ValueError as e:
    print("ValueError:", e)
