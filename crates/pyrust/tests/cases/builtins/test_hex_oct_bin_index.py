# Parity fixture: hex(), oct(), bin() call __index__ on user-defined objects.
# CPython 3.12 reference: https://docs.python.org/3.12/library/functions.html#hex

class MyInt:
    def __index__(self):
        return 42

# Basic __index__ dispatch
print(hex(MyInt()))   # 0x2a
print(oct(MyInt()))   # 0o52
print(bin(MyInt()))   # 0b101010

# Regression: plain int still works
print(hex(42))        # 0x2a
print(oct(42))        # 0o52
print(bin(42))        # 0b101010

# bool is a subclass of int
print(hex(True))      # 0x1
print(hex(False))     # 0x0
print(oct(True))      # 0o1
print(bin(True))      # 0b1

# Negative values
class Neg:
    def __index__(self):
        return -255

print(hex(Neg()))     # -0xff
print(oct(Neg()))     # -0o377
print(bin(Neg()))     # -0b11111111

# Object with no __index__ and not int: TypeError
class Foo:
    pass

try:
    hex(Foo())
except TypeError as e:
    print(e)

try:
    oct(Foo())
except TypeError as e:
    print(e)

try:
    bin(Foo())
except TypeError as e:
    print(e)

# __index__ returning non-int: TypeError
class BadIndex:
    def __index__(self):
        return "hello"

try:
    hex(BadIndex())
except TypeError as e:
    print(e)

try:
    oct(BadIndex())
except TypeError as e:
    print(e)

try:
    bin(BadIndex())
except TypeError as e:
    print(e)
