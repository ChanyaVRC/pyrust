# Parity fixture for issue #1143: user-defined __new__ is called during
# class instantiation before __init__.

# --- Basic __new__ dispatch ---

class Foo:
    def __new__(cls, x):
        print("new:" + cls.__name__)
        return super().__new__(cls)

    def __init__(self, x):
        print("init:" + str(x))
        self.x = x

f = Foo(42)
print(f.x)                      # 42

# --- Singleton pattern ---

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
print(s1 is s2)                 # True

# --- __new__ returning non-instance skips __init__ ---

class ReturnInt:
    def __new__(cls):
        print("new called")
        return 99

    def __init__(self):
        print("init should not run")

result = ReturnInt()
print(result)                   # 99
print(type(result).__name__)    # int

# --- __new__ returning different subclass: __init__ not called on wrong class ---

class Base:
    def __new__(cls):
        return super().__new__(cls)

class Child(Base):
    pass

class Factory(Base):
    def __new__(cls):
        # Return a Child instance instead of Factory
        return object.__new__(Child)

    def __init__(self):
        # Should NOT be called since result is not a Factory instance
        print("Factory.__init__ should not run")

obj = Factory()
print(type(obj).__name__)       # Child
print(isinstance(obj, Child))   # True
print(isinstance(obj, Base))    # True

# --- Inheritance chain: __new__ propagates through super() ---

class A:
    def __new__(cls, val):
        print("A.new:" + cls.__name__)
        instance = super().__new__(cls)
        return instance

    def __init__(self, val):
        self.val = val

class B(A):
    def __new__(cls, val):
        print("B.new")
        return super().__new__(cls, val)

    def __init__(self, val):
        super().__init__(val)

b = B(7)
print(b.val)                    # 7

# --- object.__new__(cls) called directly ---

class Bare:
    pass

inst = object.__new__(Bare)
print(type(inst).__name__)      # Bare
print(isinstance(inst, Bare))   # True

# --- __new__ without __init__ ---

class NoInit:
    def __new__(cls):
        return super().__new__(cls)

n = NoInit()
print(type(n).__name__)         # NoInit
print(isinstance(n, NoInit))    # True
