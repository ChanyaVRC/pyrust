# __init__ returning non-None must raise TypeError with the type name, not RuntimeError.

# Standard path (no user-defined __new__)
class Foo:
    def __init__(self):
        return 42

try:
    Foo()
except TypeError as e:
    print(e)

# User-defined __new__ path
class Bar:
    def __new__(cls):
        return super().__new__(cls)
    def __init__(self):
        return "hello"

try:
    Bar()
except TypeError as e:
    print(e)

# Returning None (or not returning) must NOT raise
class Ok:
    def __init__(self):
        return None

Ok()
print("Ok() succeeded")

class OkImplicit:
    def __init__(self):
        pass

OkImplicit()
print("OkImplicit() succeeded")

# Exception subclass must still construct correctly
class MyError(Exception):
    pass

e = MyError("msg")
print(e.args)
