# Exception subclass __str__ dispatch: CPython 3.12 parity.
# str(exc), print(exc), f"{exc}", f"{exc!s}", and format(exc, "") must all
# call the user-defined __str__ when it is defined on the exception subclass.

class MyError(Exception):
    def __str__(self):
        return "custom error message"

e = MyError("original")
print(str(e))     # custom error message
print(e)          # custom error message
print(repr(e))    # MyError('original')

# f-string bare embed: format(e, "") -> object.__format__("") -> str(e) -> __str__
print(f"{e}")     # custom error message

# !s conversion: str(e) -> __str__
print(f"{e!s}")   # custom error message

# !r conversion: repr(e) -> default repr, unaffected
print(f"{e!r}")   # MyError('original')

# format() builtin with empty spec: same as f"{e}"
print(format(e, ""))  # custom error message

# !s with a format spec: str(e) gives string, then format the string
print(f"{e!s:>21}")   # (21-wide right-align of "custom error message")

# __init__ with extra attrs, __str__ using them
class CodedError(Exception):
    def __init__(self, code, msg):
        super().__init__(msg)
        self.code = code
    def __str__(self):
        return "[" + str(self.code) + "] " + self.args[0]

e2 = CodedError(404, "not found")
print(str(e2))    # [404] not found
print(f"{e2}")    # [404] not found
print(f"{e2!s}")  # [404] not found

# Inherited user-defined __str__ from intermediate class
class Base(Exception):
    def __str__(self):
        return "base_str"

class Child(Base):
    pass

print(str(Child("y")))  # base_str
print(f"{Child('y')}")  # base_str

# Plain exception without custom __str__: default formatting preserved
print(str(ValueError("plain")))   # plain
print(str(ValueError(1, 2)))      # (1, 2)
print(f"{ValueError('plain')}")   # plain

# User class with __format__ defined: __format__ is still called, not __str__
class FmtError(Exception):
    def __str__(self):
        return "str_result"
    def __format__(self, spec):
        return "fmt_result"

fe = FmtError("x")
print(format(fe, ""))   # fmt_result  (__format__ takes priority)
print(f"{fe}")          # fmt_result
print(f"{fe!s}")        # str_result  (!s bypasses __format__ and calls str())
print(str(fe))          # str_result
