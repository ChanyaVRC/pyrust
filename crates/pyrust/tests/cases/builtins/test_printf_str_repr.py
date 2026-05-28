"""
Parity fixture: printf-style % formatting with %s and %r for user-defined
objects and primitive subclasses.

CPython 3.12:
- "%s" % obj  calls str(obj), which dispatches __str__ then __repr__
- "%r" % obj  calls repr(obj), which dispatches __repr__
- For int/float/str/bytes subclasses without a custom dunder, the backing
  primitive's str/repr is used rather than the generic object address.
"""


# --- user-defined class with __str__ and __repr__ ---

class Foo:
    def __str__(self): return "hello"
    def __repr__(self): return "<Foo>"


print("%s" % Foo())   # hello
print("%r" % Foo())   # <Foo>


# --- user-defined class with only __repr__ (no __str__) ---
# str() falls back to __repr__ when __str__ is not defined

class Bar:
    def __repr__(self): return "<Bar>"


print("%s" % Bar())   # <Bar>
print("%r" % Bar())   # <Bar>


# --- plain int, str, list: no regression ---

print("%s" % 42)      # 42
print("%s" % "hi")    # hi
print("%s" % [1, 2])  # [1, 2]


# --- int subclass without custom __str__/__repr__ ---
# Should use the int backing, not the generic address repr.

class MyInt(int):
    pass


print("%s" % MyInt(42))    # 42
print("%r" % MyInt(42))    # 42


# --- int subclass with custom __str__ ---

class Bar2(int):
    def __str__(self): return "custom"


print("%s" % Bar2(5))      # custom
print("%r" % Bar2(5))      # 5  (int's repr, since Bar2 has no __repr__)


# --- float subclass without custom dunders ---

class MyFloat(float):
    pass


print("%s" % MyFloat(3.14))   # 3.14
print("%r" % MyFloat(3.14))   # 3.14


# --- str subclass without custom dunders ---
# str() on a str subclass returns the raw string; repr() adds quotes.

class MyStr(str):
    pass


print("%s" % MyStr("hi"))     # hi
print("%r" % MyStr("hi"))     # 'hi'


# --- bytes subclass without custom dunders ---

class MyBytes(bytes):
    pass


print("%s" % MyBytes(b"hi"))  # b'hi'
print("%r" % MyBytes(b"hi"))  # b'hi'


# --- inheritance: __str__ from base class ---

class Base:
    def __str__(self): return "base"


class Child(Base):
    pass


print("%s" % Child())   # base


# --- precision: %.5s truncates the result ---

class Foo2:
    def __str__(self): return "hello world"


print("%.5s" % Foo2())   # hello


# --- precision with int subclass ---

print("%.3s" % MyInt(1234567))  # 123  (truncated str of "1234567")


# --- %r with precision ---

class Foo3:
    def __repr__(self): return "<hello world>"


print("%.5r" % Foo3())   # <hell


# --- width and alignment ---

class Foo4:
    def __str__(self): return "hi"


print("%10s" % Foo4())   #         hi
print("%-10s|" % Foo4()) # hi        |


# --- __str__ returning non-string raises TypeError ---

class BadStr:
    def __str__(self): return 42


try:
    print("%s" % BadStr())
except TypeError as e:
    print(f"TypeError: {e}")


# --- __repr__ returning non-string raises TypeError ---

class BadRepr:
    def __repr__(self): return 42


try:
    print("%r" % BadRepr())
except TypeError as e:
    print(f"TypeError: {e}")
