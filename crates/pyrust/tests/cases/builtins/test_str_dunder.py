# str() must call __str__ on user-defined exception subclasses.
# CPython 3.12 behaviour: __str__ is looked up on the type, user-defined
# takes priority over the built-in BaseException.__str__ fallback.

class MyError(Exception):
    def __str__(self):
        return "custom: " + str(self.args[0])

e = MyError("boom")
print(str(e))    # custom: boom
print(repr(e))   # MyError('boom')

try:
    raise MyError("caught")
except MyError as ex:
    print(str(ex))  # custom: caught

# Built-in exceptions without a custom __str__ still use the default formatting.
print(str(ValueError("plain")))   # plain
print(str(ValueError(1, 2)))      # (1, 2)
print(str(Exception()))           # (empty string)
print(str(Exception(1, 2, 3)))    # (1, 2, 3)

# Inherited user-defined __str__: subclass of a class that defines __str__.
class Base(Exception):
    def __str__(self):
        return "base_str"

class Child(Base):
    pass

print(str(Child("y")))  # base_str

# Exception subclass whose __str__ uses OSError context.
class MyOSError(OSError):
    def __str__(self):
        return "os: " + str(self.args[0])

print(str(MyOSError("file")))  # os: file

# str.format() must also call user __str__ on exception subclasses.
class MyFmtError(Exception):
    def __str__(self):
        return "fmt: " + str(self.args[0])

print("{}".format(MyFmtError("msg")))  # fmt: msg
