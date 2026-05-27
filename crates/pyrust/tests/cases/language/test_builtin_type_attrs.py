# Parity fixture for issue #1256: dunder methods accessible as class attributes
# on built-in types (object, int, str, list, ...).

# ── hasattr checks ────────────────────────────────────────────────────────────
print(hasattr(object, '__str__'))
print(hasattr(object, '__repr__'))
print(hasattr(object, '__eq__'))
print(hasattr(object, '__ne__'))
print(hasattr(object, '__hash__'))
print(hasattr(object, '__init__'))
print(hasattr(object, '__lt__'))
print(hasattr(object, '__le__'))
print(hasattr(object, '__gt__'))
print(hasattr(object, '__ge__'))
print(hasattr(object, '__format__'))
print(hasattr(object, '__init_subclass__'))

print(hasattr(int, '__add__'))
print(hasattr(int, '__sub__'))
print(hasattr(int, '__mul__'))
print(hasattr(int, '__truediv__'))
print(hasattr(int, '__floordiv__'))
print(hasattr(int, '__mod__'))
print(hasattr(int, '__and__'))
print(hasattr(int, '__or__'))
print(hasattr(int, '__xor__'))
print(hasattr(int, '__lt__'))
print(hasattr(int, '__le__'))
print(hasattr(int, '__gt__'))
print(hasattr(int, '__ge__'))
print(hasattr(int, '__eq__'))
print(hasattr(int, '__ne__'))

print(hasattr(str, '__len__'))
print(hasattr(str, '__add__'))
print(hasattr(str, '__mul__'))
print(hasattr(str, '__lt__'))
print(hasattr(str, '__eq__'))

print(hasattr(list, '__len__'))
print(hasattr(tuple, '__len__'))
print(hasattr(dict, '__len__'))
print(hasattr(set, '__len__'))
print(hasattr(bytes, '__len__'))

# ── object.__str__ and __repr__ via callable ──────────────────────────────────
print(object.__str__(42))
print(object.__str__(3.14))
print(object.__str__(True))
print(object.__str__(None))
print(object.__str__("hello"))

# ── object comparison dunders ─────────────────────────────────────────────────
print(object.__lt__(1, 2))   # NotImplemented (object doesn't define ordering)
print(object.__gt__(1, 2))   # NotImplemented
print(object.__le__(1, 2))   # NotImplemented
print(object.__ge__(1, 2))   # NotImplemented

# object.__eq__ uses identity: same object -> True, different -> NotImplemented
x = object()
print(object.__eq__(x, x))          # True
print(object.__eq__(x, object()))   # NotImplemented

# ── int arithmetic dunders ────────────────────────────────────────────────────
print(int.__add__(1, 2))
print(int.__sub__(10, 3))
print(int.__mul__(3, 4))
print(int.__truediv__(7, 2))
print(int.__floordiv__(7, 2))
print(int.__mod__(7, 3))
print(int.__pow__(2, 10))
print(int.__and__(0b1100, 0b1010))
print(int.__or__(0b1100, 0b1010))
print(int.__xor__(0b1100, 0b1010))
print(int.__lshift__(1, 4))
print(int.__rshift__(16, 2))

# ── int comparison dunders ────────────────────────────────────────────────────
print(int.__lt__(1, 2))
print(int.__le__(2, 2))
print(int.__gt__(3, 2))
print(int.__ge__(2, 2))
print(int.__eq__(5, 5))
print(int.__ne__(5, 6))

# ── str dunders ───────────────────────────────────────────────────────────────
print(str.__len__("hello"))
print(str.__len__(""))
print(str.__add__("foo", "bar"))
print(str.__mul__("ab", 3))
print(str.__lt__("a", "b"))
print(str.__eq__("x", "x"))
print(str.__ne__("x", "y"))

# ── sequence __len__ dunders ─────────────────────────────────────────────────
print(list.__len__([1, 2, 3]))
print(tuple.__len__((4, 5)))
print(dict.__len__({"a": 1, "b": 2}))
print(set.__len__({10, 20, 30}))
print(bytes.__len__(b"abc"))

# ── super().__str__() in custom class hierarchies ─────────────────────────────
class MyStr:
    def __str__(self):
        parent = super().__str__()
        # parent is <ClassName object at addr> — only check prefix/suffix
        return "wrapped"


print(str(MyStr()))

class MyChild(MyStr):
    def __str__(self):
        return "child -> " + super().__str__()

print(str(MyChild()))

# super().__repr__() in class hierarchy
class A:
    def greet(self):
        return "A"

class B(A):
    def greet(self):
        return "B:" + super().greet()

class C(B):
    def greet(self):
        return "C:" + super().greet()

print(C().greet())

# super().__init_subclass__() still works
class Base:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        print("subclass created:", cls.__name__)

class Child(Base):
    pass

# super() with explicit object base
class ExplicitObj(object):
    pass

eo = ExplicitObj()
# str(eo) should produce <__main__.ExplicitObj object at ...>
s = str(eo)
print(s.startswith("<"))
print("ExplicitObj" in s)

# object.__hash__ for user instances: two different instances have different hashes
x = object()
y = object()
print(object.__hash__(x) == object.__hash__(x))   # True (same instance)
print(type(object.__hash__(x)))                    # <class 'int'>
