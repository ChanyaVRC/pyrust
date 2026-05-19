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

# Regression guards: valid sequence multiplication must still work
print([1, 2] * 3)    # [1, 2, 1, 2, 1, 2]
print("ab" * 4)      # abababab
