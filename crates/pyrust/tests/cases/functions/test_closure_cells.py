# Parity fixture for issue #2106: function.__closure__ returns a tuple of cell
# objects (with .cell_contents) for a real closure, None for a non-closure;
# co_freevars aligns (sorted by name).


def make():
    a = 1
    b = "hi"

    def inner():
        return a, b

    return inner


fn = make()
# __closure__ is a tuple of cells, one per free variable.
print(type(fn.__closure__).__name__)  # tuple
print(len(fn.__closure__))  # 2
print(fn.__closure__[0].cell_contents)  # 1
print(fn.__closure__[1].cell_contents)  # hi
print(type(fn.__closure__[0]).__name__)  # cell
# co_freevars is the sorted free-variable name tuple.
print(fn.__code__.co_freevars)  # ('a', 'b')


# A function with no free variables: __closure__ is None, co_freevars is ().
def plain():
    return 1


print(plain.__closure__)  # None
print(plain.__code__.co_freevars)  # ()


# Nested closures expose the outer free var across two levels.
def outer():
    a = 10

    def middle():
        b = 20

        def innermost():
            return a + b

        return innermost

    return middle()


f = outer()
print(f.__closure__[0].cell_contents, f.__closure__[1].cell_contents)  # 10 20
print(f.__code__.co_freevars)  # ('a', 'b')


# A closure over a mutated nonlocal reflects the cell's current value.
def counter():
    n = 0

    def inc():
        nonlocal n
        n += 1
        return n

    inc()
    inc()
    return inc


g = counter()
print(g.__closure__[0].cell_contents)  # 2
print(g.__code__.co_freevars)  # ('n',)


# A name referenced as a module global is NOT a closure free variable.
G = 99


def uses_global():
    return G


print(uses_global.__closure__)  # None
print(uses_global.__code__.co_freevars)  # ()


# Free variables are reported in sorted order, both in __closure__ and
# co_freevars.
def order():
    z = 1
    a = 2
    m = 3

    def use():
        return z, a, m

    return use


use = order()
print(use.__code__.co_freevars)  # ('a', 'm', 'z')
print(tuple(c.cell_contents for c in use.__closure__))  # (2, 3, 1)


# A lambda closure and a method that captures an enclosing function local.
def mk(a):
    return lambda: a


print(mk(7).__closure__[0].cell_contents, mk(7).__code__.co_freevars)  # 7 ('a',)


def with_method():
    cfg = "C"

    class K:
        def m(self):
            return cfg

    return K


K = with_method()
print(K.m.__closure__[0].cell_contents, K.m.__code__.co_freevars)  # C ('cfg',)
