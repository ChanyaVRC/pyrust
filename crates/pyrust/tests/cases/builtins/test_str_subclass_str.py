# Parity fixture for issue #1564: str(MyStr('hi')) should return the backing
# string value when MyStr defines __repr__ but not __str__.
# CPython's str.__str__ returns self directly; it never consults __repr__.
# Only int.__str__/float.__str__ delegate to __repr__.

class MyStr(str):
    def __repr__(self): return "<mystr>"

class MyInt(int):
    def __repr__(self): return "<myint>"

class MyStrCustomStr(str):
    def __str__(self): return "custom_str"
    def __repr__(self): return "<mystr>"

class MyBytes(bytes):
    def __repr__(self): return "<mybytes>"

class MyFloat(float):
    def __repr__(self): return "<myfloat>"

class MyStrPlain(str):
    pass

class MyIntPlain(int):
    pass

class MyBytesPlain(bytes):
    pass

class MyFloatPlain(float):
    pass

# str subclass with __repr__ only: str() returns backing, repr() calls __repr__
print(str(MyStr("hi")))      # hi
print(repr(MyStr("hi")))     # <mystr>
print(MyStr("hi"))           # hi  (print uses __str__)

# int subclass with __repr__ only: str() calls __repr__ (int.__str__ delegates)
print(str(MyInt(42)))        # <myint>

# str subclass with both: __str__ takes precedence over backing
print(str(MyStrCustomStr("x")))  # custom_str

# bytes subclass with __repr__ only: str() returns backing bytes repr
print(str(MyBytes(b"hello")))    # b'hello'

# float subclass with __repr__ only: str() calls __repr__
print(str(MyFloat(3.14)))    # <myfloat>

# plain subclasses (no dunders): each returns backing value str
print(str(MyStrPlain("test")))    # test
print(str(MyIntPlain(99)))        # 99
print(str(MyBytesPlain(b"abc")))  # b'abc'
print(str(MyFloatPlain(1.5)))     # 1.5
