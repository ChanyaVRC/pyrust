# Parity fixture for issue #1112: user-defined __init__ on exception subclasses.
# CPython reference: BaseException.__init__ updates .args; user __init__ sets
# instance attributes.

# Basic case: user __init__ with super().__init__
class MyError(ValueError):
    def __init__(self, msg, code):
        super().__init__(msg)
        self.code = code

e = MyError("bad input", 42)
print(e.code)
print(str(e))
print(e.args)

# No custom __init__: plain subclass still works
class NoInit(RuntimeError):
    pass

e2 = NoInit("plain")
print(str(e2))
print(e2.args)

# Multiple args forwarded to super
class MultiArg(Exception):
    def __init__(self, a, b):
        super().__init__(a, b)
        self.a = a
        self.b = b

e3 = MultiArg(1, 2)
print(e3.a)
print(e3.b)
print(e3.args)

# User __init__ without calling super: args stays as constructor args
class NoSuper(TypeError):
    def __init__(self, msg, detail):
        self.detail = detail

e4 = NoSuper("msg", "detail here")
print(e4.detail)
print(e4.args)

# Deep inheritance chain
class Base(Exception):
    def __init__(self, x):
        super().__init__(x)
        self.x = x

class Derived(Base):
    def __init__(self, x, y):
        super().__init__(x)
        self.y = y

d = Derived(10, 20)
print(d.x)
print(d.y)
print(d.args)

# StopIteration subclass: .value updated by super().__init__
class MyStop(StopIteration):
    def __init__(self, val, meta):
        super().__init__(val)
        self.meta = meta

s = MyStop("done", "info")
print(s.value)
print(s.meta)
print(s.args)

# raise and catch user exception subclass
class AppError(RuntimeError):
    def __init__(self, msg, detail):
        super().__init__(msg)
        self.detail = detail

try:
    raise AppError("fail", "details here")
except AppError as ex:
    print(ex.detail)
    print(str(ex))

# Keyword argument in user __init__
class KwError(Exception):
    def __init__(self, msg, *, code=0):
        super().__init__(msg)
        self.code = code

e5 = KwError("err", code=99)
print(e5.code)
print(str(e5))
