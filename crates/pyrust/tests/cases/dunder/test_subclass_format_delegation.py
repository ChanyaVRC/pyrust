# Builtin-subclass __format__ delegation: super()-proxy form (#2211),
# method-call form (#2214), and the actual-subclass name in the
# unsupported-spec TypeError (#2212).  All three must match CPython 3.12.


# --- #2211: super().__format__(spec) with a non-empty spec on a primitive
# subclass delegates to the backing type's __format__.
class I(int):
    def __format__(self, spec):
        return "I[" + super().__format__(spec) + "]"


print(format(I(255), "x"))   # I[ff]
print(format(I(255), ""))    # I[255]
print(format(I(10), "05d"))  # I[00010]


class MyStr(str):
    def __format__(self, spec):
        return "<" + super().__format__(spec) + ">"


print(format(MyStr("hi"), ">5"))  # <   hi>
print(f"{MyStr('hi'):>5}")        # <   hi>


# --- #2214: inst.__format__(spec) method-call form on a primitive subclass
# (no override) delegates to the backing, exactly like format(inst, spec).
class I2(int):
    pass


class S(str):
    pass


class F(float):
    pass


print(I2(255).__format__("x"))      # ff
print(I2(255).__format__("b"))      # 11111111
print(I2(5).__format__(""))         # 5
print(S("hi").__format__(">5"))     # "   hi"
print(F(3.14159).__format__(".2f")) # 3.14
print((255).__format__("x"))        # ff (plain primitive unaffected)


# --- #2212: the unsupported-spec TypeError names the actual subclass, not the
# backing primitive, across the method, format(), f-string and str.format paths.
class B(bytes):
    pass


for label, fn in [
    ("method", lambda: B(b"hi").__format__("x")),
    ("format", lambda: format(B(b"hi"), "x")),
    ("fstring", lambda: f"{B(b'hi'):x}"),
    ("strfmt", lambda: "{:x}".format(B(b"hi"))),
]:
    try:
        fn()
    except TypeError as e:
        print(label, e)


# Arg-validation errors name the backing type (int), matching CPython's MRO
# resolution to int.__format__.
try:
    I2(5).__format__("x", "y")
except TypeError as e:
    print("toomany", e)
try:
    I2(5).__format__(spec="x")
except TypeError as e:
    print("kw", e)
try:
    I2(5).__format__(5)
except TypeError as e:
    print("nonstr", e)


# A user override is still respected over the backing delegation.
class Over(int):
    def __format__(self, spec):
        return "OVERRIDE"


print(Over(9).__format__("x"))  # OVERRIDE
print(format(Over(9), "d"))     # OVERRIDE


# Empty-spec on a non-formattable subclass falls back to str(self).
class L(list):
    pass


print(L([1, 2]).__format__(""))  # [1, 2]
