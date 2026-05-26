# Parity fixture for issue #1285: bool() must validate __len__() return value.
# CPython 3.12 raises ValueError for negative returns and OverflowError for
# values that don't fit in an index-sized integer.

class ZeroLen:
    def __len__(self): return 0

class PosLen:
    def __len__(self): return 5

class NegLen:
    def __len__(self): return -1

class TooLarge:
    def __len__(self): return 2**200

class NegBigInt:
    def __len__(self): return -(2**200)

class BoolTrueLen:
    def __len__(self): return True

class BoolFalseLen:
    def __len__(self): return False

class FloatLen:
    def __len__(self): return 3.5

class StrLen:
    def __len__(self): return "hello"

# Happy path: zero is falsy, positive is truthy.
try:
    print(bool(ZeroLen()))
except Exception as e:
    print(type(e).__name__ + ':', e)

try:
    print(bool(PosLen()))
except Exception as e:
    print(type(e).__name__ + ':', e)

# Bool returns from __len__ are accepted (True == 1, False == 0).
try:
    print(bool(BoolTrueLen()))
except Exception as e:
    print(type(e).__name__ + ':', e)

try:
    print(bool(BoolFalseLen()))
except Exception as e:
    print(type(e).__name__ + ':', e)

# Negative int raises ValueError.
try:
    print(bool(NegLen()))
except ValueError as e:
    print('ValueError:', e)

# BigInt too large for an index raises OverflowError.
try:
    print(bool(TooLarge()))
except OverflowError as e:
    print('OverflowError:', e)

# Negative BigInt raises ValueError.
try:
    print(bool(NegBigInt()))
except ValueError as e:
    print('ValueError:', e)

# Non-integer types raise TypeError.
try:
    print(bool(FloatLen()))
except TypeError as e:
    print('TypeError:', e)

try:
    print(bool(StrLen()))
except TypeError as e:
    print('TypeError:', e)

# Indirect bool usage: if statement, boolean operators, any/all.
obj = NegLen()

try:
    if obj:
        print("truthy")
except ValueError as e:
    print('if ValueError:', e)

try:
    print(obj or "default")
except ValueError as e:
    print('or ValueError:', e)

try:
    print(obj and "yes")
except ValueError as e:
    print('and ValueError:', e)

try:
    print(any([obj]))
except ValueError as e:
    print('any ValueError:', e)

try:
    print(all([obj]))
except ValueError as e:
    print('all ValueError:', e)
