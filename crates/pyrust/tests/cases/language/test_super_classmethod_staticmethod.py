# Tests for super(), classmethod, and staticmethod

# ─── staticmethod ─────────────────────────────────────────────────────────────

class MathUtils:
    @staticmethod
    def add(a, b):
        return a + b

    @staticmethod
    def multiply(a, b):
        return a * b

print(MathUtils.add(3, 4))          # 7
print(MathUtils.multiply(5, 6))     # 30

# staticmethod via instance
mu = MathUtils()
print(mu.add(10, 20))               # 30

# ─── classmethod ──────────────────────────────────────────────────────────────

class Counter:
    count = 0

    @classmethod
    def increment(cls):
        cls.count += 1

    @classmethod
    def get_count(cls):
        return cls.count

    @classmethod
    def from_value(cls, value):
        c = cls()
        c.value = value
        return c

Counter.increment()
Counter.increment()
Counter.increment()
print(Counter.get_count())          # 3

# classmethod via instance
c = Counter()
c.increment()
print(Counter.get_count())          # 4

# classmethod as factory
obj = Counter.from_value(42)
print(obj.value)                    # 42

# ─── classmethod receives the actual class (subclasses) ───────────────────────

class Animal:
    name = "Animal"

    @classmethod
    def speak(cls):
        return cls.name

class Dog(Animal):
    name = "Dog"

print(Animal.speak())               # Animal
print(Dog.speak())                  # Dog

# ─── super() two-argument form ────────────────────────────────────────────────

class Base:
    def greet(self):
        return "Hello from Base"

    def value(self):
        return 10

class Child(Base):
    def greet(self):
        base_greet = super(Child, self).greet()
        return base_greet + " (via Child)"

    def value(self):
        return super(Child, self).value() + 5

c = Child()
print(c.greet())                    # Hello from Base (via Child)
print(c.value())                    # 15

# ─── super() in __init__ ──────────────────────────────────────────────────────

class Shape:
    def __init__(self, color):
        self.color = color

class Circle(Shape):
    def __init__(self, color, radius):
        super(Circle, self).__init__(color)
        self.radius = radius

    def describe(self):
        return self.color + " circle r=" + str(self.radius)

circle = Circle("red", 5)
print(circle.describe())            # red circle r=5
print(circle.color)                 # red
print(circle.radius)                # 5

# ─── multi-level inheritance with super() ─────────────────────────────────────

class A:
    def method(self):
        return "A"

class B(A):
    def method(self):
        return "B->" + super(B, self).method()

class C(B):
    def method(self):
        return "C->" + super(C, self).method()

obj = C()
print(obj.method())                 # C->B->A

# ─── classmethod combined with inheritance ────────────────────────────────────

class Base2:
    @classmethod
    def create(cls):
        return cls()

class Sub2(Base2):
    def describe(self):
        return "I am Sub2"

s = Sub2.create()
print(s.describe())                 # I am Sub2

# ─── staticmethod does not receive self/cls ───────────────────────────────────

class Validator:
    @staticmethod
    def is_positive(n):
        return n > 0

    def check(self, n):
        return Validator.is_positive(n)

v = Validator()
print(v.check(5))                   # True
print(v.check(-1))                  # False
print(Validator.is_positive(0))     # False
