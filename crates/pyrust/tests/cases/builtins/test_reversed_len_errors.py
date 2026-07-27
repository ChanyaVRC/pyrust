"""Parity fixture: reversed() error handling for bad __len__ return values.

CPython 3.12 raises:
  - TypeError "'X' object cannot be interpreted as an integer" for non-int __len__
  - OverflowError "cannot fit 'int' into an index-sized integer" for very large int __len__
"""

# Case 1: __len__ returns float -> TypeError with correct message
class BadLenFloat:
    def __len__(self):
        return 1.5
    def __getitem__(self, i):
        return i

try:
    list(reversed(BadLenFloat()))
except TypeError as e:
    print("TypeError:", e)
except Exception as e:
    print(type(e).__name__ + ":", e)

# Case 2: __len__ returns a value too large for index -> OverflowError
class BigLen:
    def __len__(self):
        return 2**63
    def __getitem__(self, i):
        return i

try:
    list(reversed(BigLen()))
except OverflowError as e:
    print("OverflowError:", e)
except TypeError as e:
    print("TypeError:", e)
except Exception as e:
    print(type(e).__name__ + ":", e)

# Case 3: __len__ returns a user instance -> TypeError with class name
class MyObj:
    pass

class BadLenInstance:
    def __len__(self):
        return MyObj()
    def __getitem__(self, i):
        return i

try:
    list(reversed(BadLenInstance()))
except TypeError as e:
    print("TypeError:", e)
except Exception as e:
    print(type(e).__name__ + ":", e)

# Case 4: normal sequence reversed() still works
print(list(reversed([1, 2, 3])))
print(list(reversed(range(4))))

# Case 5: __len__ returns bool (True=1) -> works
class BoolLen:
    def __len__(self):
        return True
    def __getitem__(self, i):
        return i * 10

print(list(reversed(BoolLen())))

# Case 6: __len__ returns negative int -> ValueError
class NegLen:
    def __len__(self):
        return -1
    def __getitem__(self, i):
        return i

try:
    list(reversed(NegLen()))
except ValueError as e:
    print("ValueError:", e)
except Exception as e:
    print(type(e).__name__ + ":", e)

# Case 7: __len__ returns negative BigInt -> ValueError (not OverflowError)
class NegBigLen:
    def __len__(self):
        return -(2**200)
    def __getitem__(self, i):
        return i

try:
    list(reversed(NegBigLen()))
except ValueError as e:
    print("ValueError:", e)
except Exception as e:
    print(type(e).__name__ + ":", e)

# Case 8: reversed() uses the same __index__ normalization as len()/bool().
class IndexLength:
    def __index__(self):
        return 3

class IndexLenSequence:
    def __len__(self):
        return IndexLength()
    def __getitem__(self, i):
        return i * 10

print(list(reversed(IndexLenSequence())))

# Case 9: a direct int-subclass result is an integer length; its __index__
# override is not invoked because the object already is an int.
class LengthInt(int):
    def __index__(self):
        return 99

class IntSubclassLenSequence:
    def __len__(self):
        return LengthInt(2)
    def __getitem__(self, i):
        return i

print(list(reversed(IntSubclassLenSequence())))
