# del e.args raises TypeError on BaseException and all subclasses (issue #1601)

# Exception
e = Exception("test")
try:
    del e.args
except TypeError as e2:
    print(type(e2).__name__, str(e2))

# ValueError
e = ValueError("val")
try:
    del e.args
except TypeError as e2:
    print(type(e2).__name__, str(e2))

# SyntaxError
e = SyntaxError("syn")
try:
    del e.args
except TypeError as e2:
    print(type(e2).__name__, str(e2))

# User-defined subclass
class MyError(Exception):
    pass

e = MyError("my")
try:
    del e.args
except TypeError as e2:
    print(type(e2).__name__, str(e2))

# e.args is still settable
e = Exception("test")
e.args = (1, 2)
print(e.args)

# BaseException itself (not only subclasses)
e = BaseException("base")
try:
    del e.args
except TypeError as e2:
    print(type(e2).__name__, str(e2))

# object.__delattr__ path also raises TypeError
e = Exception("x")
try:
    object.__delattr__(e, "args")
except TypeError as e2:
    print(type(e2).__name__, str(e2))

# del on non-exception user class — AttributeError for nonexistent attr
class Foo:
    pass

f = Foo()
try:
    del f.args
except AttributeError:
    print("AttributeError ok")
