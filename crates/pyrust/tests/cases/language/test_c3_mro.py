# Parity fixture for C3 MRO linearization (issue #1233).
# Uses __name__ strings to avoid repr differences between implementations.

# --- Basic diamond ---
class A: pass
class B(A): pass
class C(A): pass
class D(B, C): pass

print([c.__name__ for c in D.__mro__])

# --- Single inheritance ---
class P: pass
class Q(P): pass
print([c.__name__ for c in Q.__mro__])

# --- No explicit base ---
class R: pass
print([c.__name__ for c in R.__mro__])

# --- type.mro() returns a list ---
print([c.__name__ for c in type.mro(D)])

# --- Complex multi-level diamond ---
class O: pass
class A2(O): pass
class B2(O): pass
class C2(A2, B2): pass
class D2(A2, B2): pass
class E2(C2, D2): pass
print([c.__name__ for c in E2.__mro__])

# --- Cooperative multiple inheritance via super() ---
class Base:
    def method(self):
        return "Base"

class Left(Base):
    def method(self):
        return "Left+" + super().method()

class Right(Base):
    def method(self):
        return "Right+" + super().method()

class Child(Left, Right):
    pass

print(Child().method())

# --- super().__init__ chains through diamond ---
class BaseI:
    def __init__(self):
        self.base = True

class MixinI:
    def __init__(self):
        super().__init__()
        self.mixin = True

class ChildI(MixinI, BaseI):
    def __init__(self):
        super().__init__()

c = ChildI()
print(c.base, c.mixin)

# --- Inconsistent MRO raises TypeError at class creation ---
class X: pass
class Y(X): pass
try:
    class Z(X, Y): pass
    print("ERROR: no TypeError")
except TypeError as e:
    # Print just the first line to avoid platform-specific newline handling.
    print("TypeError:", e.args[0].split("\n")[0])
