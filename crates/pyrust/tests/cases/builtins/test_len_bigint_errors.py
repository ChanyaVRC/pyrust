"""Parity fixture: len() error handling for bad __len__ return values.

CPython 3.12 raises:
  - OverflowError "cannot fit 'int' into an index-sized integer" for positive BigInt __len__
  - ValueError "__len__() should return >= 0" for negative __len__ (int or BigInt)
  - TypeError "'X' object cannot be interpreted as an integer" for non-int __len__
"""

# Case 1: __len__ returns a positive BigInt (too large for isize) -> OverflowError
class BigLen:
    def __len__(self):
        return 2**63
    def __getitem__(self, i):
        return i

try:
    len(BigLen())
except OverflowError as e:
    print("OverflowError:", e)
except Exception as e:
    print(type(e).__name__ + ":", e)

# Case 2: __len__ returns a negative BigInt -> ValueError
class NegBigLen:
    def __len__(self):
        return -(2**63)

try:
    len(NegBigLen())
except ValueError as e:
    print("ValueError:", e)
except Exception as e:
    print(type(e).__name__ + ":", e)

# Case 3: __len__ returns float -> TypeError with correct message
class FloatLen:
    def __len__(self):
        return 1.5

try:
    len(FloatLen())
except TypeError as e:
    print("TypeError:", e)
except Exception as e:
    print(type(e).__name__ + ":", e)

# Case 4: normal len() still works
print(len([1, 2, 3]))
print(len("hello"))
print(len(()))

# Case 5: negative int (not BigInt) -> ValueError
class NegSmallLen:
    def __len__(self):
        return -1

try:
    len(NegSmallLen())
except ValueError as e:
    print("ValueError:", e)
except Exception as e:
    print(type(e).__name__ + ":", e)
