# Regression fixture for issue #438.
#
# Before the fix, `pass_algebraic_simplify` rewrote `x + 0`, `x * 1`, `x * 0`,
# `x ** 0`, `x ** 1`, `x - 0` into Move / LoadConst at compile time, bypassing
# the `__add__` / `__sub__` / `__mul__` / `__pow__` dunder dispatch on user
# classes.
#
# This file exercises each of the six identity patterns with a user-class LHS
# (the failure case is *inside* a function body — at module scope the
# optimizer's register allocation usually prevents the unsafe rewrite).

class C:
    def __add__(self, other):
        return ("add", other)
    def __mul__(self, other):
        return ("mul", other)
    def __pow__(self, other):
        return ("pow", other)
    def __sub__(self, other):
        return ("sub", other)


def f_add(x):
    return x + 0

def f_sub(x):
    return x - 0

def f_mul1(x):
    return x * 1

def f_mul0(x):
    return x * 0

def f_pow0(x):
    return x ** 0

def f_pow1(x):
    return x ** 1


c = C()
print(f_add(c))     # ('add', 0)
print(f_sub(c))     # ('sub', 0)
print(f_mul1(c))    # ('mul', 1)
print(f_mul0(c))    # ('mul', 0)
print(f_pow0(c))    # ('pow', 0)
print(f_pow1(c))    # ('pow', 1)

# Primitive ints still produce numerically-correct results.
print(f_add(7))     # 7
print(f_sub(7))     # 7
print(f_mul1(7))    # 7
print(f_mul0(7))    # 0
print(f_pow0(7))    # 1
print(f_pow1(7))    # 7

# Constant LHS continues to be folded by pass_const_fold.
def const_fold_check():
    return 5 + 0
print(const_fold_check())   # 5
