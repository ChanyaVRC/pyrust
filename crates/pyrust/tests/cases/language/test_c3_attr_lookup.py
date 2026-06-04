# Parity fixture for issue #2075: class attribute/method lookup must follow
# the C3 MRO, not a depth-first walk of the base chain.  In a diamond
# `D(B, C)` where `C` overrides an attribute inherited (not defined) by `B`,
# CPython scans the MRO `D, B, C, A` left-to-right and returns C's value.

# --- Data attribute resolved via C3 (sibling override beats common ancestor) ---
class A:
    x = "A"
class B(A):
    pass
class C(A):
    x = "C"
class D(B, C):
    pass

print([c.__name__ for c in D.__mro__])  # ['D', 'B', 'C', 'A', 'object']
print(D.x)                              # C
print(D().x)                            # C

# --- Method resolved via C3 ---
class A2:
    def m(self):
        return "A2"
class B2(A2):
    pass
class C2(A2):
    def m(self):
        return "C2"
class D2(B2, C2):
    pass

print(D2().m())                         # C2

# --- Inherit-only (no override anywhere) still finds the ancestor value ---
class A3:
    y = "Ay"
class B3(A3):
    pass
class C3(A3):
    pass
class D3(B3, C3):
    pass

print(D3.y)                             # Ay

# --- classmethod resolution through a diamond ---
class M1:
    @classmethod
    def cm(cls):
        return cls.__name__
class M2(M1):
    pass
class M3(M1):
    @classmethod
    def cm(cls):
        return "M3:" + cls.__name__
class M4(M2, M3):
    pass

print(M4.cm())                          # M3:M4

# --- Attribute only on the second base ---
class X:
    pass
class Y:
    val = 99
class Z(X, Y):
    pass

print(Z.val)                            # 99

# --- super() still cooperates through the diamond (unchanged) ---
class Base:
    def who(self):
        return ["Base"]
class Left(Base):
    def who(self):
        return ["Left"] + super().who()
class Right(Base):
    def who(self):
        return ["Right"] + super().who()
class Child(Left, Right):
    def who(self):
        return ["Child"] + super().who()

print(Child().who())                    # ['Child', 'Left', 'Right', 'Base']

# --- Multi-level diamond: E inherits from D, attribute on a far sibling ---
class P:
    tag = "P"
class Q1(P):
    pass
class Q2(P):
    tag = "Q2"
class Rr(Q1, Q2):
    pass
class S(Rr):
    pass

print([c.__name__ for c in S.__mro__])  # ['S', 'Rr', 'Q1', 'Q2', 'P', 'object']
print(S.tag)                            # Q2

# --- Single inheritance is unchanged ---
class Pa:
    k = 1
    def f(self):
        return "Pa.f"
class Qa(Pa):
    pass

print(Qa.k, Qa().f())                   # 1 Pa.f
