# type(name, bases, dict) with multiple bases: all bases must be in the MRO
# and their methods / class attributes must be accessible on instances.
# CPython 3.12: type('C', (A, B), {}) creates a class inheriting from both A
# and B.  pyrust must match (issue #1064).

class A:
    def method_a(self): return "a"

class B:
    def method_b(self): return "b"

# --- type() 3-arg with two bases ---
C = type("C", (A, B), {})
c = C()
print(c.method_a())        # a
print(c.method_b())        # b
print(issubclass(C, A))    # True
print(issubclass(C, B))    # True
print(isinstance(c, A))    # True
print(isinstance(c, B))    # True

# --- Methods from dict take priority over inherited methods ---
C2 = type("C2", (A, B), {"method_a": lambda self: "overridden"})
print(C2().method_a())     # overridden
print(C2().method_b())     # b

# --- Single base: no regression ---
D = type("D", (A,), {})
print(D().method_a())      # a
print(issubclass(D, A))    # True

# --- No bases: defaults to object ---
E = type("E", (), {})
print(issubclass(E, object)) # True

# --- class statement with two bases ---
class X:
    x_attr = "x"

class Y:
    y_attr = "y"

class Z(X, Y):
    pass

z = Z()
print(z.x_attr)            # x
print(z.y_attr)            # y
print(issubclass(Z, X))    # True
print(issubclass(Z, Y))    # True

# --- Three bases ---
class P:
    p_val = 1
class Q:
    q_val = 2
class R:
    r_val = 3

S = type("S", (P, Q, R), {})
s = S()
print(s.p_val)             # 1
print(s.q_val)             # 2
print(s.r_val)             # 3
print(issubclass(S, P))    # True
print(issubclass(S, Q))    # True
print(issubclass(S, R))    # True

# --- MRO includes all bases ---
mro_names = [cls.__name__ for cls in C.__mro__]
print(mro_names[0])        # C
print("A" in mro_names)    # True
print("B" in mro_names)    # True
print("object" in mro_names) # True

# --- Mixin pattern: class-level attribute from extra base ---
class Mixin:
    extra = True

Base = type("Base", (object,), {})
Combined = type("Combined", (Base, Mixin), {})
print(Combined.extra)      # True
