# Test instantiation and basic operations for bare subclasses of primitive types.
# Issue #1204 / #1347: MyStr("hello") and friends previously raised
# "RuntimeError: MyStr() takes no arguments".

class MyInt(int):
    pass

class MyStr(str):
    pass

class MyFloat(float):
    pass

class MyBytes(bytes):
    pass

# --- Construction ---
x = MyInt(42)
s = MyStr("hello")
f = MyFloat(3.14)
b = MyBytes(b"ab")

# type() identity and isinstance
print(type(x).__name__)        # MyInt
print(type(x) is MyInt)        # True
print(isinstance(x, int))      # True
print(isinstance(x, MyInt))    # True

print(type(s).__name__)        # MyStr
print(isinstance(s, str))      # True

print(type(f).__name__)        # MyFloat
print(isinstance(f, float))    # True

print(type(b).__name__)        # MyBytes
print(isinstance(b, bytes))    # True

# --- Printing (str representation) ---
print(x)   # 42
print(s)   # hello
print(f)   # 3.14
print(b)   # b'ab'

# --- Arithmetic on MyInt ---
print(x + 1)   # 43
print(x - 1)   # 41
print(x * 2)   # 84
print(x // 5)  # 8
print(x % 5)   # 2
print(-x)      # -42
print(+x)      # 42
print(~x)      # -43
print(x ** 2)  # 1764
print(abs(x))  # 42

# --- Comparisons on MyInt ---
print(MyInt(5) == 5)    # True
print(MyInt(5) != 6)    # True
print(MyInt(5) < 10)    # True
print(MyInt(5) > 10)    # False
print(MyInt(5) <= 5)    # True
print(MyInt(5) >= 5)    # True

# --- Arithmetic on MyFloat ---
print(f + 1.0)  # 4.140000000000001
print(-f)       # -3.14
print(abs(f))   # 3.14

# --- str methods on MyStr ---
print(s.upper())          # HELLO
print(s + " world")       # hello world
print(len(s))             # 5
print(s[0])               # h
print(s.startswith("h"))  # True

# --- bytes methods on MyBytes ---
print(len(b))    # 2
print(b + b"c")  # b'abc'

# --- bool() truthiness ---
print(bool(MyInt(0)))   # False
print(bool(MyInt(1)))   # True
print(bool(MyStr("")))  # False
print(bool(MyStr("x"))) # True
print(not MyInt(0))     # True
print(not MyInt(5))     # False

# --- Bitwise operations on MyInt ---
print(MyInt(5) | 2)    # 7
print(MyInt(6) & 3)    # 2
print(MyInt(5) ^ 3)    # 6
print(MyInt(4) >> 1)   # 2
print(MyInt(4) << 1)   # 8

# --- Inherited int methods ---
print(MyInt(255).bit_length())  # 8

# --- Custom __init__ alongside inherited construction ---
class MyInt2(int):
    def __init__(self, val):
        self.extra = "extra"

m = MyInt2(10)
print(m)        # 10
print(m.extra)  # extra

# --- Zero / negative MyInt ---
print(MyInt(0))    # 0
print(MyInt(-7))   # -7
print(abs(MyInt(-7)))  # 7

# --- repr() delegates to backing primitive ---
print(repr(MyInt(42)))      # 42
print(repr(MyStr("hello"))) # 'hello'
print(repr(MyFloat(3.14)))  # 3.14
print(repr(MyBytes(b"ab"))) # b'ab'

# --- Casting back to base type ---
print(int(MyInt(42)))         # 42
print(float(MyFloat(3.14)))   # 3.14
print(str(MyStr("hello")))    # hello
print(bytes(MyBytes(b"ab")))  # b'ab'
