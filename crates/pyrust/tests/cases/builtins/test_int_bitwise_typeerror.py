# CPython 3.12 parity: int bitwise ops with non-int RHS raise correct TypeError
try:
    print(42 | {1, 2})
except TypeError as e:
    print(e)

try:
    print(42 & [1])
except TypeError as e:
    print(e)

try:
    print(42 ^ "x")
except TypeError as e:
    print(e)

try:
    print(True | {1})
except TypeError as e:
    print(e)

try:
    print(42 << "x")
except TypeError as e:
    print(e)

try:
    print(42 >> [1])
except TypeError as e:
    print(e)

# Happy path: int bitwise ops with int/bool still work
print(42 | 5)
print(42 & 15)
print(42 ^ 15)
print(1 << 4)
print(64 >> 2)
print(42 | True)

# Negative shift raises ValueError, not TypeError
try:
    42 << -1
except ValueError as e:
    print(e)

try:
    42 >> -1
except ValueError as e:
    print(e)
