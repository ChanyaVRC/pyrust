# Parity fixture: range() raises TypeError for non-integer arguments.
# CPython 3.12 reference: TypeError: '<type>' object cannot be interpreted as an integer

try:
    range(1.5)
except TypeError as e:
    print(type(e).__name__ + ":", e)

try:
    range("a")
except TypeError as e:
    print(type(e).__name__ + ":", e)

try:
    range(1, 2.0)
except TypeError as e:
    print(type(e).__name__ + ":", e)

try:
    range(1, 2, 0.5)
except TypeError as e:
    print(type(e).__name__ + ":", e)

# ValueError for zero step is unchanged
try:
    range(1, 2, 0)
except ValueError as e:
    print(type(e).__name__ + ":", e)

# Happy path: valid range still works
print(list(range(0, 10, 2)))
print(list(range(5)))
print(list(range(1, 4)))
