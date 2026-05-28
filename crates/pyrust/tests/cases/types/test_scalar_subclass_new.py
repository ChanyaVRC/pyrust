# Issue #1465: super().__new__(cls, val) in int/str/float/bytes subclasses
# must populate the __builtin_data__ backing store so the instance behaves
# like the primitive type rather than printing the generic object repr.

class MyInt(int):
    def __new__(cls, val):
        return super().__new__(cls, val)

x = MyInt(5)
print(x)                         # 5
print(MyInt(5) + MyInt(3))       # 8
print(type(MyInt(5)).__name__)   # MyInt
print(isinstance(MyInt(5), int)) # True


class MyStr(str):
    def __new__(cls, val):
        return super().__new__(cls, val)

y = MyStr("hello")
print(y)                          # hello
print(y.upper())                  # HELLO
print(type(y).__name__)           # MyStr
print(isinstance(y, str))         # True


class MyFloat(float):
    def __new__(cls, val):
        return super().__new__(cls, val)

z = MyFloat(3.14)
print(z)                          # 3.14
print(type(z).__name__)           # MyFloat
print(isinstance(z, float))       # True


class MyBytes(bytes):
    def __new__(cls, val):
        return super().__new__(cls, val)

b = MyBytes(b"hi")
print(b)                          # b'hi'
print(type(b).__name__)           # MyBytes
print(isinstance(b, bytes))       # True


# A __new__ that returns a plain int (not super().__new__) is unaffected.
class MyIntPlain(int):
    def __new__(cls, val):
        return int(val)

p = MyIntPlain(7)
print(p)                          # 7
print(type(p).__name__)           # int  (not MyIntPlain — returned plain int)


# Plain int/str/float/bytes construction (no custom __new__) must still work.
print(int(42))    # 42
print(str(99))    # 99
print(float(1))   # 1.0
print(bytes(3))   # b'\x00\x00\x00'
