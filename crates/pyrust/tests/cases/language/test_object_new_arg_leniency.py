class Foo:
    def __init__(self, x):
        self.x = x

f = Foo(42)
print(f.x)  # 42

class Bar:
    def __new__(cls, x):
        return super().__new__(cls)
    def __init__(self, x):
        self.x = x

b = Bar(10)
print(b.x)  # 10

# object.__new__ accepts extra args when cls defines __init__
class HasInit:
    def __init__(self, x):
        self.x = x

obj = object.__new__(HasInit, 42)
print(type(obj).__name__)  # HasInit

# object.__new__ accepts extra kwargs when cls defines __init__
class HasInit2:
    def __init__(self, x, y=0):
        pass

obj2 = object.__new__(HasInit2, 1, y=2)
print(type(obj2).__name__)  # HasInit2

# object.__new__ rejects extra args when cls has no custom __init__
class Plain:
    pass

try:
    object.__new__(Plain, "extra")
except TypeError as e:
    print("TypeError raised:", "ok")

# object.__new__ rejects extra kwargs when cls has no custom __init__
try:
    object.__new__(object, x=1)
except TypeError as e:
    print("TypeError raised:", "ok")

# object.__new__ with only cls arg always works
obj3 = object.__new__(HasInit)
print(type(obj3).__name__)  # HasInit
obj4 = object.__new__(Plain)
print(type(obj4).__name__)  # Plain
