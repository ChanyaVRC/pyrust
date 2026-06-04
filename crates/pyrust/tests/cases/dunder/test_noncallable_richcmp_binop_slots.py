# Issue #2055: a non-callable rich-comparison / binary-operator / unary slot
# (`__eq__ = 5`, `__add__ = "x"`, `__neg__ = 5`, `__hash__ = 5`) raises
# `TypeError: '<type>' object is not callable` when the operator is used —
# matching CPython, which invokes the slot and discovers it is non-callable.
# (#1963 fixed the protocol-call paths; this extends the same to the rich-cmp /
# binop / unary / hash slot dispatch routes.)


def check(setup, expr):
    ns = {}
    exec(setup, ns)
    try:
        print(eval(expr, ns))
    except TypeError as e:
        print("TypeError:", e)


# Rich comparison slots.
check("class D:\n  __eq__ = 5", "D() == D()")
check("class D:\n  __ne__ = 5", "D() != D()")
check("class D:\n  __lt__ = 'x'", "D() < D()")
check("class D:\n  __le__ = 5", "D() <= D()")
check("class D:\n  __gt__ = 5", "D() > D()")
check("class D:\n  __ge__ = 5", "D() >= D()")

# Binary-operator slots (forward and reflected).
check("class A:\n  __add__ = 5", "A() + A()")
check("class A:\n  __sub__ = 5", "A() - A()")
check("class A:\n  __mul__ = 5", "A() * A()")
check("class A:\n  __matmul__ = 5", "A() @ A()")
check("class A:\n  __radd__ = 5", "1 + A()")

# Unary slots.
check("class A:\n  __neg__ = 5", "-A()")
check("class A:\n  __pos__ = 5", "+A()")
check("class A:\n  __invert__ = 5", "~A()")

# Hash slot.
check("class A:\n  __hash__ = 5", "hash(A())")

# Callable slots are unaffected.
class Ok:
    def __eq__(self, other):
        return True

    def __add__(self, other):
        return 99


print(Ok() == Ok())
print(Ok() + Ok())
