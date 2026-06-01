# Exercises every branch of the env-lookup rule documented in
# helpers.rs (issue #452): global read/write, nonlocal across multiple
# enclosing scopes, free-variable capture, module-scope fall-through, and the
# synthetic __class__ cell used by super(). Behaviour must be identical
# before/after the consolidation refactor.

g = 10


def use_global():
    global g
    g = g + 5
    return g


def free_var_capture():
    x = 1

    def inner():
        # `x` is a free variable: rule 3 parent-walk capture.
        return x + 1

    return inner()


def nonlocal_two_levels():
    count = 0

    def mid():
        def deep():
            nonlocal count  # binds to outer `count`, skipping `mid`
            count += 1
            return count

        return deep()

    a = mid()
    b = mid()
    return a, b, count


def shadow_then_nonlocal():
    v = "outer"

    def reader():
        return v  # free-var read of outer v

    def writer():
        nonlocal v
        v = "changed"

    before = reader()
    writer()
    after = reader()
    return before, after


class Base:
    def who(self):
        return "base"


class Derived(Base):
    def who(self):
        # super() relies on the synthetic __class__ cell lookup.
        return "derived+" + super().who()


print(use_global())
print(g)
print(free_var_capture())
print(nonlocal_two_levels())
print(shadow_then_nonlocal())
print(Derived().who())

# Module-scope name read from inside a function (rule 3 bottoming out at the
# module env), plus an UnboundLocalError path.
MODCONST = 99


def reads_module_const():
    return MODCONST * 2


print(reads_module_const())


def touches_unbound():
    try:
        print(y)  # local (assigned below) but read before assignment
        y = 1
    except UnboundLocalError as e:
        return "unbound:" + str(type(e).__name__)


print(touches_unbound())
