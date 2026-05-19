# CPython 3.12 parity: sequence * non-int emits type-named TypeError

try:
    result = (1, 2) * None
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'NoneType'

try:
    result = (1, 2) * "x"
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'str'

try:
    result = [1, 2] * [3]
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'list'

try:
    result = None * (1, 2)
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'NoneType'

try:
    result = b"ab" * None
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'NoneType'

# float case (also generalised by this fix)
try:
    result = (1, 2) * 1.5
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'float'

# Bool is int subtype — multiplication still works
print((1, 2) * True)   # (1, 2)
print([1, 2] * False)  # []

# int multiplication still works
print([1, 2] * 3)   # [1, 2, 1, 2, 1, 2]
print("ab" * 4)     # abababab

# Tuple * Int and Bytes * Int positive cases (self-review catches: were missing arms)
print((1, 2) * 3)   # (1, 2, 1, 2, 1, 2)
print(3 * (1, 2))   # (1, 2, 1, 2, 1, 2)
print(b"ab" * 2)    # b"abab"
print(2 * b"ab")    # b"abab"
print((1, 2) * 0)   # ()
print((1, 2) * -1)  # ()
