import sys

# Default limit exists and is an int
limit = sys.getrecursionlimit()
print(type(limit).__name__)  # int
print(limit > 0)             # True

# Can set and retrieve
sys.setrecursionlimit(500)
print(sys.getrecursionlimit())  # 500

# Reset
sys.setrecursionlimit(1000)
print(sys.getrecursionlimit())  # 1000

# Invalid value raises ValueError
try:
    sys.setrecursionlimit(-1)
except ValueError as e:
    print(type(e).__name__)  # ValueError
    print(str(e))            # recursion limit must be greater or equal than 1

# 0 is also invalid (CPython 3.12 requires >= 1)
try:
    sys.setrecursionlimit(0)
except ValueError as e:
    print(type(e).__name__)  # ValueError

# Non-integer raises TypeError
try:
    sys.setrecursionlimit("100")
except TypeError as e:
    print(type(e).__name__)  # TypeError

try:
    sys.setrecursionlimit(1.5)
except TypeError as e:
    print(type(e).__name__)  # TypeError

# Arg-count TypeError messages include the qualified name "sys.*"
try:
    sys.getrecursionlimit(1)
except TypeError as e:
    print(str(e))  # sys.getrecursionlimit() takes no arguments (1 given)

try:
    sys.setrecursionlimit()
except TypeError as e:
    print(str(e))  # sys.setrecursionlimit() takes exactly one argument (0 given)

try:
    sys.setrecursionlimit(100, 200)
except TypeError as e:
    print(str(e))  # sys.setrecursionlimit() takes exactly one argument (2 given)

# setrecursionlimit raises RecursionError when new limit <= current call depth
try:
    sys.setrecursionlimit(1)  # current depth >= 1 at top-level
except RecursionError as e:
    print(type(e).__name__)  # RecursionError
