# CPython 3.12 parity fixture for sequence * non-int TypeError messages

try:
    result = (1, 2) * 1.5
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'float'

try:
    result = [1, 2] * 1.5
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'float'

try:
    result = "ab" * 1.5
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'float'

try:
    result = b"ab" * 1.5
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'float'

try:
    result = 1.5 * (1, 2)
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'float'

try:
    result = 1.5 * [1, 2]
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'float'

try:
    result = 1.5 * "ab"
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'float'

try:
    result = 1.5 * b"ab"
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'float'

# Non-float non-int types also raise the type-named TypeError (issue #756 full fix)
try:
    result = [1, 2] * None
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'NoneType'

try:
    result = None * [1, 2]
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'NoneType'

try:
    result = "ab" * "x"
except TypeError as e:
    print(e)  # can't multiply sequence by non-int of type 'str'

# Regression guards: valid sequence multiplication must still work
print([1, 2] * 3)    # [1, 2, 1, 2, 1, 2]
print("ab" * 4)      # abababab
# bool is a subclass of int — should be accepted
print([1, 2] * True)   # [1, 2]
print([1, 2] * False)  # []
