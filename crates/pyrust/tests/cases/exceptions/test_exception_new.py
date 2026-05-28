# Issue #1420: user-defined __new__ on exception subclasses must be called.

# Basic: __new__ with side-effect (append to list)
class MyError(Exception):
    _instances = []
    def __new__(cls, msg):
        inst = super().__new__(cls, msg)
        cls._instances.append(inst)
        return inst

e = MyError("oops")
print(type(e).__name__)        # MyError
print(len(MyError._instances)) # 1
print(str(e))                  # oops

# Inherited __new__ through exception subclass chain
class Base(Exception):
    _count = 0
    def __new__(cls, *args):
        Base._count += 1
        return super().__new__(cls, *args)

class Child(Base):
    pass

Child("test")
print(Base._count)  # 1

# __new__ returning a non-instance skips __init__
class ReturnInt(Exception):
    def __new__(cls, msg):
        return 42

result = ReturnInt("x")
print(type(result).__name__)  # int
print(result)                  # 42

# __new__ and __init__ both defined — both should be called
class BothDefined(Exception):
    _created = []
    def __new__(cls, msg):
        inst = super().__new__(cls, msg)
        cls._created.append("new")
        return inst

    def __init__(self, msg):
        super().__init__(msg)
        self._created.append("init")

b = BothDefined("hi")
print(b._created)  # ['new', 'init']
print(str(b))       # hi

# Plain exception (no custom __new__) still works
e2 = ValueError("plain")
print(type(e2).__name__)  # ValueError
print(str(e2))             # plain
