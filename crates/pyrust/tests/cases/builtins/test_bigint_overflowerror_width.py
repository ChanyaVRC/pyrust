# Parity fixture for issue #1636:
# BigInt width/tabsize arguments must raise OverflowError, not TypeError.
# CPython 3.12 reference messages:
#   center/ljust/rjust: "Python int too large to convert to C ssize_t"
#   expandtabs:         "Python int too large to convert to C int"

BIG = 2**100

# str methods — width args → OverflowError: C ssize_t
try:
    "hello".center(BIG)
except OverflowError as e:
    print("str.center OverflowError:", e)

try:
    "hello".ljust(BIG)
except OverflowError as e:
    print("str.ljust OverflowError:", e)

try:
    "hello".rjust(BIG)
except OverflowError as e:
    print("str.rjust OverflowError:", e)

try:
    "hello".zfill(BIG)
except OverflowError as e:
    print("str.zfill OverflowError:", e)

# str.expandtabs — tabsize arg → OverflowError: C int
try:
    "hello".expandtabs(BIG)
except OverflowError as e:
    print("str.expandtabs OverflowError:", e)

# bytes methods — width args → OverflowError: C ssize_t
try:
    b"hello".center(BIG)
except OverflowError as e:
    print("bytes.center OverflowError:", e)

try:
    b"hello".ljust(BIG)
except OverflowError as e:
    print("bytes.ljust OverflowError:", e)

try:
    b"hello".rjust(BIG)
except OverflowError as e:
    print("bytes.rjust OverflowError:", e)

# bytes.expandtabs — tabsize arg → OverflowError: C int
try:
    b"a\tb".expandtabs(BIG)
except OverflowError as e:
    print("bytes.expandtabs OverflowError:", e)

# Bool args still work (bool is a subclass of int in Python)
print("hi".center(True))
print(b"hi".ljust(True))
print("a\tb".expandtabs(True))
