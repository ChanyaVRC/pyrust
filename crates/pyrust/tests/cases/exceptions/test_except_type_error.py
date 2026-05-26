# except clause with a non-exception type must raise TypeError (not RuntimeError)
# CPython 3.12: "catching classes that do not inherit from BaseException is not allowed"

# Case 1: bare integer in except clause
try:
    try:
        raise ValueError("x")
    except 42:
        pass
except TypeError as e:
    print("TypeError:", e)

# Case 2: tuple with non-exception element
try:
    try:
        raise ValueError("x")
    except (ValueError, 42):
        pass
except TypeError as e:
    print("TypeError:", e)

# Case 3: string in except clause
try:
    try:
        raise ValueError("x")
    except "oops":
        pass
except TypeError as e:
    print("TypeError:", e)

# Case 4: non-exception class in except clause
class NotAnException:
    pass

try:
    try:
        raise ValueError("x")
    except NotAnException:
        pass
except TypeError as e:
    print("TypeError:", e)

# Positive: valid exception class works fine
try:
    raise ValueError("hello")
except ValueError as e:
    print("caught:", e)

# Positive: user-defined exception subclass works
class MyErr(Exception):
    pass

try:
    raise MyErr("world")
except MyErr as e:
    print("caught user exc:", e)

# Positive: tuple of valid exception classes works
try:
    raise TypeError("t")
except (ValueError, TypeError) as e:
    print("caught tuple:", type(e).__name__)
