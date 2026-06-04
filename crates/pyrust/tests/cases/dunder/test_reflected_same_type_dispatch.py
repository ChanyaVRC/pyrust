# Issue #2092: when both operands of a binary arithmetic operator are the
# *same* type and the forward dunder returns NotImplemented, CPython does NOT
# call the reflected dunder on the right operand (the reflected slot is
# identical to the forward one already tried) — it raises TypeError.  The
# reflected slot is still tried for proper subclasses that override it, and for
# unrelated types.


def show(expr, fn):
    try:
        print(repr(fn()))
    except Exception as ex:
        print(expr, "->", type(ex).__name__, ex)


# --- same type, forward NotImplemented, reflected defined -> TypeError -------
class S:
    def __add__(self, o):
        return NotImplemented

    def __radd__(self, o):
        return "S.radd"


show("S() + S()", lambda: S() + S())


class Sm:
    def __sub__(self, o):
        return NotImplemented

    def __rsub__(self, o):
        return "Sm.rsub"


show("Sm() - Sm()", lambda: Sm() - Sm())


class Mm:
    def __mul__(self, o):
        return NotImplemented

    def __rmul__(self, o):
        return "Mm.rmul"


show("Mm() * Mm()", lambda: Mm() * Mm())


class Dm:
    def __truediv__(self, o):
        return NotImplemented

    def __rtruediv__(self, o):
        return "Dm.rtruediv"


show("Dm() / Dm()", lambda: Dm() / Dm())


class Pm:
    def __pow__(self, o):
        return NotImplemented

    def __rpow__(self, o):
        return "Pm.rpow"


show("Pm() ** Pm()", lambda: Pm() ** Pm())


class MM:
    def __matmul__(self, o):
        return NotImplemented

    def __rmatmul__(self, o):
        return "MM.rmatmul"


show("MM() @ MM()", lambda: MM() @ MM())


# --- different types: reflected IS tried ------------------------------------
class L:
    def __add__(self, o):
        return NotImplemented


class R:
    def __radd__(self, o):
        return "R.radd"


show("L() + R()", lambda: L() + R())


# --- subclass overriding reflected gets priority ----------------------------
class Base:
    def __add__(self, o):
        return "Base.add"


class Sub(Base):
    def __radd__(self, o):
        return "Sub.radd"


show("Base() + Sub()", lambda: Base() + Sub())


# --- subclass where forward returns NotImplemented --------------------------
class B2:
    def __add__(self, o):
        return NotImplemented

    def __radd__(self, o):
        return "B2.radd"


class S2(B2):
    def __radd__(self, o):
        return "S2.radd"


show("B2() + S2()", lambda: B2() + S2())
show("S2() + B2()", lambda: S2() + B2())
show("B2() + B2()", lambda: B2() + B2())
show("S2() + S2()", lambda: S2() + S2())


# --- forward succeeds: reflected never consulted (same type) ----------------
class F:
    def __add__(self, o):
        return "F.add"

    def __radd__(self, o):
        return "F.radd"


show("F() + F()", lambda: F() + F())


# --- comparisons are unaffected (reflected name is not __r*) ----------------
class Cmp:
    def __lt__(self, o):
        return NotImplemented

    def __gt__(self, o):
        return "Cmp.gt"


show("Cmp() < Cmp()", lambda: Cmp() < Cmp())
