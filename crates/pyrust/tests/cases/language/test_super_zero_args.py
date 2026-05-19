# Test zero-argument super() — CPython 3.12 parity (issue #733).
#
# CPython synthesises a `__class__` cell variable for every function compiled
# directly inside a class body.  Zero-arg super() uses that cell plus the
# first positional parameter (self/cls) to construct the super proxy.
#
# Rules:
#   - super() inside a direct method resolves to the enclosing class + self.
#   - super() inside a classmethod resolves to the enclosing class + cls.
#   - super() inside a nested function (def inner inside a method) raises
#     RuntimeError because inner() is not a direct class method.
#   - super() at module scope or in a plain function raises RuntimeError.
#   - Explicit super(Type, obj) always works regardless of context.

# Basic single-inheritance
class A:
    def greet(self):
        return "A"

class B(A):
    def greet(self):
        return super().greet() + "B"

print(B().greet())  # AB

# Multi-level inheritance
class C(B):
    def greet(self):
        return super().greet() + "C"

print(C().greet())  # ABC

# super() in a classmethod
class Base:
    @classmethod
    def make(cls):
        return "Base"

class Child(Base):
    @classmethod
    def make(cls):
        return super().make() + "+Child"

print(Child.make())  # Base+Child

# Explicit super(Type, obj) still works
class X:
    def val(self):
        return 10

class Y(X):
    def val(self):
        return super(Y, self).val() + 1

print(Y().val())  # 11

# super() from a nested function inside a method raises RuntimeError
class M:
    def method(self):
        def inner():
            super()
        inner()

try:
    M().method()
    print("FAIL: expected RuntimeError")
except RuntimeError as e:
    print(f"RuntimeError: {e}")  # RuntimeError: super(): no arguments

# super() at module scope raises RuntimeError
try:
    super()
    print("FAIL: expected RuntimeError")
except RuntimeError as e:
    print(f"RuntimeError: {e}")  # RuntimeError: super(): no arguments

# super() in a plain function (not a method) raises RuntimeError
def standalone():
    super()

try:
    standalone()
    print("FAIL: expected RuntimeError")
except RuntimeError as e:
    print(f"RuntimeError: {e}")  # RuntimeError: super(): no arguments

# super() works correctly in nested class definitions
class Outer:
    class Inner(A):
        def greet(self):
            return "Inner+" + super().greet()

print(Outer.Inner().greet())  # Inner+A
