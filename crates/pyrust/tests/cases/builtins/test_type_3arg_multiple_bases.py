# Parity fixture for type(name, bases, dict) multiple-inheritance path.
# Verifies issue #1064: extra bases beyond the first were silently dropped.
# Covers isinstance/issubclass, MRO order, and diamond inheritance.

class A:
    def a(self): return 'a'

class B:
    def b(self): return 'b'

# Basic multiple inheritance via type() 3-arg form.
C = type('C', (A, B), {})
c = C()
print(c.a())          # a
print(c.b())          # b

# isinstance checks against all bases.
print(isinstance(c, A))   # True
print(isinstance(c, B))   # True
print(isinstance(c, C))   # True

# issubclass checks against all bases.
print(issubclass(C, A))   # True
print(issubclass(C, B))   # True
print(issubclass(C, object))  # True

# MRO includes all bases in linearisation order.
mro_names = [cls.__name__ for cls in C.__mro__]
print(mro_names[0])        # C
print('A' in mro_names)    # True
print('B' in mro_names)    # True
print('object' in mro_names)  # True

# Attributes from the namespace dict shadow inherited ones.
D = type('D', (A, B), {'a': lambda self: 'overridden'})
print(D().a())        # overridden
print(D().b())        # b

# Class-level attribute from extra base is reachable.
class Mixin:
    extra = True

Base = type('Base', (object,), {})
Combined = type('Combined', (Base, Mixin), {})
print(Combined.extra)  # True

# Diamond inheritance: A and B both inherit from Root; D inherits A and B.
# C3 linearisation must visit each class only once.
class Root:
    def who(self): return 'root'

class Left(Root):
    def who(self): return 'left'

class Right(Root):
    pass

Diamond = type('Diamond', (Left, Right), {})
d = Diamond()
print(d.who())               # left  (Left takes priority over Root via MRO)
print(isinstance(d, Left))   # True
print(isinstance(d, Right))  # True
print(isinstance(d, Root))   # True

mro_d = [cls.__name__ for cls in Diamond.__mro__]
print(mro_d[0])              # Diamond
print(mro_d[1])              # Left
print(mro_d[2])              # Right
print(mro_d[3])              # Root
print(mro_d[4])              # object

# Single base: regression guard.
E = type('E', (A,), {})
print(E().a())         # a
print(issubclass(E, A))   # True

# Empty bases: defaults to object only.
F = type('F', (), {'val': 42})
print(F.val)           # 42
print(issubclass(F, object))  # True

# type('X', (A, B), {}) and class G(A, B): pass have the same base MRO.
class G(A, B):
    pass

G_mro = [cls.__name__ for cls in G.__mro__[1:]]
C_mro = [cls.__name__ for cls in C.__mro__[1:]]
print(G_mro == C_mro)  # True
