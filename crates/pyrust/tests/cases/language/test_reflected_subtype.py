# Parity fixture for CPython subtype priority rule on reflected binary operators.
#
# CPython's binary_op1 (Objects/abstract.c) gives the right operand's reflected
# method priority over the left operand's forward method when the right operand's
# type is a *proper* subtype of the left operand's type AND the right operand's
# class directly defines the reflected method (not merely inherits it).

# --- Basic case from issue #1129 ---

class A:
    def __add__(self, other):
        return "A.__add__"

class B(A):
    def __radd__(self, other):
        return "B.__radd__"

print(A() + B())   # B.__radd__ (subtype priority: B directly defines __radd__)
print(B() + A())   # A.__add__ then B.__radd__... A.__add__ wins (B is not subtype of A)

# --- Child with inherited-only __radd__ does NOT get priority ---

class Base:
    def __add__(self, other):
        return "base_add"
    def __radd__(self, other):
        return "base_radd"

class Child(Base):
    pass  # inherits __radd__, does not directly define it

print(Base() + Child())   # base_add (Child inherits same __radd__ slot as Base, no priority)

# --- Child inherits a DIFFERENT __radd__ from an intermediate class ---
# CPython slot check: slotw != slotv even though Child doesn't directly define __radd__

class GrandBase:
    def __add__(self, other):
        return "grandbase_add"

class Mid(GrandBase):
    def __radd__(self, other):
        return "mid_radd"

class Leaf(Mid):
    pass  # inherits __radd__ from Mid, which differs from GrandBase's (None)

print(GrandBase() + Leaf())   # mid_radd (Leaf's inherited slot differs from GrandBase's None)

# --- Child with directly-defined __radd__ DOES get priority ---

class Parent:
    def __add__(self, other):
        return "parent_add"
    def __radd__(self, other):
        return "parent_radd"

class Sub(Parent):
    def __radd__(self, other):
        return "sub_radd"

print(Parent() + Sub())   # sub_radd (Sub directly defines __radd__)
print(Sub() + Parent())   # parent_add (Sub is not subtype of Parent here)

# --- Same class: no proper subtype, normal order applies ---

class Sym:
    def __add__(self, other):
        return "sym_add"
    def __radd__(self, other):
        return "sym_radd"

print(Sym() + Sym())   # sym_add (same class, not proper subtype)

# --- All arithmetic reflected ops get subtype priority ---

class Left:
    def __sub__(self, other): return NotImplemented
    def __mul__(self, other): return NotImplemented
    def __truediv__(self, other): return NotImplemented
    def __floordiv__(self, other): return NotImplemented
    def __mod__(self, other): return NotImplemented
    def __pow__(self, other): return NotImplemented
    def __and__(self, other): return NotImplemented
    def __or__(self, other): return NotImplemented
    def __xor__(self, other): return NotImplemented
    def __lshift__(self, other): return NotImplemented
    def __rshift__(self, other): return NotImplemented
    def __matmul__(self, other): return NotImplemented

class Right(Left):
    def __rsub__(self, other): return "rsub"
    def __rmul__(self, other): return "rmul"
    def __rtruediv__(self, other): return "rtruediv"
    def __rfloordiv__(self, other): return "rfloordiv"
    def __rmod__(self, other): return "rmod"
    def __rpow__(self, other): return "rpow"
    def __rand__(self, other): return "rand"
    def __ror__(self, other): return "ror"
    def __rxor__(self, other): return "rxor"
    def __rlshift__(self, other): return "rlshift"
    def __rrshift__(self, other): return "rrshift"
    def __rmatmul__(self, other): return "rmatmul"

l = Left()
r = Right()
print(l - r)
print(l * r)
print(l / r)
print(l // r)
print(l % r)
print(l ** r)
print(l & r)
print(l | r)
print(l ^ r)
print(l << r)
print(l >> r)
print(l @ r)

# --- right.__radd__ returns NotImplemented: fallback to left.__add__ ---

class Fallback:
    def __add__(self, other):
        return "fallback_add"

class NoResult(Fallback):
    def __radd__(self, other):
        return NotImplemented

print(Fallback() + NoResult())   # fallback_add
