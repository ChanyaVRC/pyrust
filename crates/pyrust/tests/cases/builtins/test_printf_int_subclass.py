"""
Parity fixture: printf-style % formatting with int subclasses and __index__.

CPython 3.12 coerces arguments for integer format codes (%d/%i/%u/%o/%x/%X)
via the int backing value (for int subclasses) or __index__ (for objects
defining it).  Float format codes (%e/%f/%g etc.) similarly accept float
subclasses, int subclasses, and objects with __float__ or __index__.
"""


class MyInt(int):
    pass


class MyFloat(float):
    pass


class MyIdx:
    def __index__(self):
        return 42


class BigIdx:
    def __index__(self):
        return 10**30


class MyNum:
    def __float__(self):
        return 3.14


class BadFloat:
    def __float__(self):
        return "hello"


class BadIdx:
    def __index__(self):
        return "hello"


class Foo:
    pass


# --- int subclass: small value ---
print("%d" % MyInt(42))
print("%i" % MyInt(42))
print("%u" % MyInt(42))
print("%o" % MyInt(42))
print("%x" % MyInt(42))
print("%X" % MyInt(42))

# --- int subclass: large value (BigInt backing) ---
print("%d" % MyInt(10**30))
print("%x" % MyInt(255))
print("%o" % MyInt(255))
print("%X" % MyInt(255))

# --- __index__ protocol ---
print("%d" % MyIdx())
print("%x" % MyIdx())
print("%o" % MyIdx())
print("%X" % MyIdx())

# --- __index__ returning large int ---
print("%d" % BigIdx())

# --- float format codes with float subclass ---
print("%f" % MyFloat(3.14))
print("%e" % MyFloat(3.14))
print("%g" % MyFloat(3.14))

# --- float format codes with __float__ ---
print("%f" % MyNum())
print("%e" % MyNum())
print("%g" % MyNum())

# --- float format codes with __index__ (int acceptable as float) ---
print("%f" % MyIdx())

# --- error: object with neither int backing nor __index__ (%d) ---
try:
    "%d" % Foo()
except TypeError as e:
    print(f"TypeError: {e}")

# --- error: __index__ returning non-int for %d falls back to outer error ---
try:
    "%d" % BadIdx()
except TypeError as e:
    print(f"TypeError: {e}")

# --- error: float rejected for %x/%o/%X ---
try:
    "%x" % 3.5
except TypeError as e:
    print(f"TypeError: {e}")

# --- error: __float__ returning non-float for %f ---
try:
    "%f" % BadFloat()
except TypeError as e:
    print(f"TypeError: {e}")

# --- error: __index__ returning non-int for %f propagates ---
try:
    "%f" % BadIdx()
except TypeError as e:
    print(f"TypeError: {e}")

# --- error: object with no __float__/__index__ for %f ---
try:
    "%f" % Foo()
except TypeError as e:
    print(f"TypeError: {e}")
