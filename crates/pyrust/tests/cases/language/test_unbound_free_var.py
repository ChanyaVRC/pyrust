# Issue #2340: reading an unbound *free* (cell) variable raises NameError with
# CPython 3.12's "cannot access free variable ... in enclosing scope" wording,
# while a plain unbound *local* keeps UnboundLocalError.  UnboundLocalError is a
# subclass of NameError, so the class identity + message is what diverges.


# Free variable read before the enclosing scope assigns it -> NameError.
def outer():
    def inner():
        return x  # x is a cell var of `outer`, captured (free) by `inner`

    try:
        inner()
    except NameError as e:
        print(type(e).__name__)
        print(str(e))
        print(isinstance(e, NameError))
        print(isinstance(e, UnboundLocalError))
    x = 1  # makes x a cell var of `outer`


outer()


# Plain local read before local assignment -> UnboundLocalError (unchanged).
def plain_local():
    try:
        return y
    except UnboundLocalError as e:
        print(type(e).__name__)
        print(str(e))
    y = 1


plain_local()


# Reading a free variable whose cell was del-eted in the enclosing scope.
def deleted_cell():
    z = 5

    def reader():
        return z

    del z
    try:
        reader()
    except NameError as e:
        print(type(e).__name__)
        print(str(e))


deleted_cell()


# Explicit `nonlocal` read of a del-eted enclosing binding -> NameError.
def nonlocal_deleted():
    w = 1

    def reader():
        nonlocal w
        return w

    del w
    try:
        reader()
    except NameError as e:
        print(type(e).__name__)
        print(str(e))


nonlocal_deleted()


# Three-level closure: the innermost reads a free variable owned by the
# outermost scope; the message wording is scope-independent.
def level_a():
    def level_b():
        def level_c():
            return q

        return level_c()

    try:
        level_b()
    except NameError as e:
        print(type(e).__name__)
        print(str(e))
    q = 1


level_a()


# A free variable that IS bound in the enclosing scope resolves normally.
def bound_free():
    v = 42

    def reader():
        return v

    print(reader())


bound_free()


# Lambda capturing an unbound free variable.
def lambda_free():
    f = lambda: k
    try:
        f()
    except NameError as e:
        print(type(e).__name__)
        print(str(e))
    k = 7


lambda_free()


# Class scope: a method reading a name that is a cell var of the enclosing
# function, unbound at call time -> NameError (free variable).
def class_scope():
    class C:
        def m(self):
            return c

    try:
        C().m()
    except NameError as e:
        print(type(e).__name__)
        print(str(e))
    c = 1


class_scope()


# A method reading a genuinely undefined name (no enclosing binding) -> the
# ordinary "name ... is not defined" NameError.
class Plain:
    def m(self):
        return undefined_global


try:
    Plain().m()
except NameError as e:
    print(type(e).__name__)
    print(str(e))


# PEP 709: a list / set / dict comprehension is inlined into the enclosing
# frame, so an unbound read of an enclosing local stays UnboundLocalError.
def list_comp_local():
    try:
        return [n for _ in range(1)]
    except UnboundLocalError as e:
        print(type(e).__name__)
        print(str(e))
    n = 1


list_comp_local()


def dict_comp_local():
    try:
        return {m: 1 for _ in range(1)}
    except UnboundLocalError as e:
        print(type(e).__name__)
        print(str(e))
    m = 1


dict_comp_local()


# A generator expression is a real separate frame (not inlined), so an unbound
# read of an enclosing local is a free variable -> NameError.
def gen_exp_free():
    try:
        return list(g for _ in range(1))
    except NameError as e:
        print(type(e).__name__)
        print(str(e))
    g = 1


gen_exp_free()
