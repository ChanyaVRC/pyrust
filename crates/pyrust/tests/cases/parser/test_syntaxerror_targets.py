# Compile-time SyntaxError for invalid assignment / delete targets, reserved
# `__debug__`, star imports, bare starred return/yield, and duplicate PEP 695
# type parameters (issues #2143, #2145, #2148).


def check(src, mode="exec"):
    try:
        compile(src, "<test>", mode)
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg)


# del of constants / literals / __debug__ (#2148).
check("del None")
check("del True")
check("del False")
check("del ...")
check("del 5")
check("del 'str'")
check("del __debug__")
check("del a, None")

# assignment to __debug__ (#2143).
check("__debug__ = 1")
check("__debug__ += 1")
check("__debug__: int = 1")
check("(__debug__ := 1)", "eval")

# import * outside module level (#2143).
check("def f():\n from math import *")
check("class C:\n from math import *")

# bare starred expression in return / yield (#2145).
check("def g(): return *x")
check("def h():\n yield *x")

# duplicate PEP 695 type parameters (#2145).
check("class C[T, T]: pass")
check("def f[T, T](): pass")
check("type X[T, T] = int")

# Valid neighbors.
xs = [1, 2, 3]
del xs[0]
print(xs)
obj = type("O", (), {})()
obj.attr = 5
del obj.attr
name = 7
del name


def starred_tuple():
    a = (1, 2)
    return (*a,)


print(starred_tuple())


class Gen[T]:
    pass


def gen_fn[T, U]():
    return None


print("targets ok")
