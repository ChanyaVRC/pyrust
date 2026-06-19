NI = NotImplemented


# When an explicit __ne__ returns NotImplemented and the reflected __ne__ also
# defers, CPython falls back to identity (a is not b), NOT to negating __eq__.
class C:
    def __eq__(self, other):
        return True

    def __ne__(self, other):
        return NI


c1, c2 = C(), C()
print(c1 != c2)  # True  (different objects -> identity)
print(c1 != c1)  # False (same object -> identity)


# A reflected user __ne__ on the right operand is tried before identity.
class D:
    def __ne__(self, other):
        return "right_ne"


print(C() != D())  # right_ne


# The default-negation path (no user __ne__ defined) must keep negating __eq__.
class E:
    def __eq__(self, other):
        return True


e = E()
print(e != e)  # False
print(e != E())  # False


# Left operand inherits object.__ne__ (only __eq__ defined), right has a user
# __ne__ returning NotImplemented.  object.__ne__ negates __eq__ single-sided,
# so the forward step decides immediately.
class A:
    def __eq__(self, other):
        return True


class B:
    def __ne__(self, other):
        return NI


print(A() != B())  # False


# Left user __ne__ returns NI; right inherits object.__ne__ with a truthy __eq__.
class L:
    def __eq__(self, other):
        return True

    def __ne__(self, other):
        return NI


print(L() != A())  # False


# Both sides' __eq__/__ne__ defer; reflected object.__ne__ negates the other's
# __eq__.
class S:
    def __eq__(self, o):
        return NI

    def __ne__(self, o):
        return NI


class O:
    def __eq__(self, o):
        return True


print(S() != O())  # False


# A subclass that redefines __ne__ gets reflected priority.
class Base:
    def __ne__(self, o):
        return "base_ne"


class Sub(Base):
    def __ne__(self, o):
        return "sub_ne"


print(Base() != Sub())  # sub_ne
print(Sub() != Base())  # sub_ne


# __ne__ returns NI on both, identical object -> identity says equal.
class F:
    def __eq__(self, o):
        return False

    def __ne__(self, o):
        return NI


f = F()
print(f != f)  # False


# Plain primitives and container element-wise dispatch must not regress.
print(1 != 1)  # False
print(1 != 2)  # True
print("a" != "b")  # True


class G:
    def __eq__(self, o):
        return isinstance(o, G)


print([G()] != [G()])  # False
