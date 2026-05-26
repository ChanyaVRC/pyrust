# CPython 3.12: exception C-level slots do not appear in __dict__.
# Issue #1152.

# Basic: args not in __dict__
e = ValueError("test")
print(e.__dict__)           # {}
print(e.args)               # ('test',)
print("args" in e.__dict__)  # False

# Custom attrs DO appear
e.custom = "value"
print(e.__dict__)           # {'custom': 'value'}
print(e.args)               # ('test',)

# __cause__ and __context__ not in __dict__
try:
    raise ValueError("inner")
except ValueError as inner:
    try:
        raise TypeError("outer") from inner
    except TypeError as outer:
        print("__cause__" in outer.__dict__)    # False
        print("__context__" in outer.__dict__)  # False
        print(outer.__dict__)                   # {}
        print(outer.args)                       # ('outer',)

# StopIteration: value not in __dict__
si = StopIteration(42)
print(si.__dict__)          # {}
print(si.value)             # 42
print("value" in si.__dict__)  # False

# SystemExit: code not in __dict__
se = SystemExit(1)
print(se.__dict__)          # {}
print(se.code)              # 1
print("code" in se.__dict__)   # False

# SyntaxError: structured attrs not in __dict__
synerr = SyntaxError("bad syntax", ("file.py", 1, 3, "x = "))
print(synerr.__dict__)      # {}
print(synerr.msg)           # bad syntax
print(synerr.filename)      # file.py

# OSError: errno/strerror/filename not in __dict__
oe = OSError(2, "No such file", "foo.txt")
print(oe.__dict__)          # {}
print(oe.errno)             # 2
print(oe.strerror)          # No such file
print(oe.filename)          # foo.txt

# ImportError: name/path not in __dict__
ie = ImportError("no module")
print(ie.__dict__)          # {}

# Custom exception with __init__ setting attrs
class MyError(Exception):
    def __init__(self, code, msg):
        super().__init__(msg)
        self.code = code

err = MyError(404, "not found")
print(err.__dict__)         # {'code': 404}
print(err.args)             # ('not found',)

# Assigning args directly still works (args itself not in __dict__)
e2 = ValueError("x")
e2.args = (1, 2)
print(e2.args)              # (1, 2)
print(e2.__dict__)          # {}

# vars() matches __dict__
e3 = ValueError("z")
e3.tag = "hello"
print(vars(e3))             # {'tag': 'hello'}
