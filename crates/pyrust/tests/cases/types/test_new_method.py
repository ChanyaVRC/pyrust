# Tests for user-defined __new__ dispatch (issue #1143).
# All output is ASCII so Windows CI (cp1252) handles it correctly.


# 1. Basic __new__ is called before __init__
class Basic:
    def __new__(cls, x):
        print("__new__ called")
        return super().__new__(cls)

    def __init__(self, x):
        print("__init__ called")
        self.x = x


b = Basic(10)
print(b.x)


# 2. Singleton pattern: __new__ returns existing instance
class Singleton:
    _instance = None

    def __new__(cls):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __init__(self):
        pass


s1 = Singleton()
s2 = Singleton()
print(s1 is s2)


# 3. __new__ returning wrong type: __init__ must NOT be called
class WrongType:
    def __new__(cls):
        print("__new__ called")
        return 42  # not an instance of WrongType

    def __init__(self):
        print("__init__ called")  # must not print


result = WrongType()
print(result)


# 4. __new__ returning an instance of a subclass: __init__ IS called
class Parent:
    def __new__(cls, value):
        print("Parent.__new__ called")
        instance = super().__new__(cls)
        instance.value = value
        return instance

    def __init__(self, value):
        print("Parent.__init__ called")


p = Parent(99)
print(p.value)


# 5. Subclass inherits __new__ from parent via MRO
class Child(Parent):
    def __init__(self, value):
        print("Child.__init__ called")
        super().__init__(value)


c = Child(7)
print(c.value)
