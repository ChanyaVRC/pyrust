# del SyntaxError special attrs resets to None (issue #1588)

# del msg resets to None
e = SyntaxError("test")
e.msg = "hello"
print(e.msg)      # hello
del e.msg
print(e.msg)      # None

# del filename resets to None
e2 = SyntaxError("test")
e2.filename = "foo.py"
del e2.filename
print(e2.filename)  # None

# del lineno resets to None
e3 = SyntaxError("test")
e3.lineno = 42
del e3.lineno
print(e3.lineno)  # None

# del offset resets to None
e4 = SyntaxError("test")
e4.offset = 5
del e4.offset
print(e4.offset)  # None

# del text resets to None
e5 = SyntaxError("test")
e5.text = "some code"
del e5.text
print(e5.text)  # None

# del end_lineno resets to None
e6 = SyntaxError("test")
e6.end_lineno = 7
del e6.end_lineno
print(e6.end_lineno)  # None

# del end_offset resets to None
e7 = SyntaxError("test")
e7.end_offset = 3
del e7.end_offset
print(e7.end_offset)  # None

# del on a never-explicitly-set slot also resets to None (no error)
e8 = SyntaxError()
del e8.msg
print(e8.msg)  # None

# object.__delattr__ also resets to None
e9 = SyntaxError("test")
e9.msg = "bye"
object.__delattr__(e9, "msg")
print(e9.msg)  # None

# del on non-special attribute raises AttributeError
e10 = SyntaxError("test")
try:
    del e10.nonexistent
except AttributeError:
    print("AttributeError ok")

# Subclass also resets to None
class MySyntaxError(SyntaxError):
    pass

e11 = MySyntaxError("test")
e11.msg = "sub"
del e11.msg
print(e11.msg)  # None
