# Parity fixture for #1918 / #1987 — parameter→register binding and the
# removal of pure-function memoization.
#
# #1918: positional arguments are now bound to registers by a compile-time
#        index (ParamBind) rather than hashing the parameter name per call.
#        Exercise every binding form so a wrong index/order would diverge from
#        CPython output.
# #1987: the pure-function result memoization (fn_cache) was removed.  A pure
#        function that branches on argument type, or one whose recursion was
#        previously memoized, must still return correct results.


# --- Positional binding, varying arg counts and orders ---
def f3(a, b, c):
    return (a, b, c)


print(f3(1, 2, 3))
print(f3(c=3, a=1, b=2))
print(f3(1, c=3, b=2))


def f8(a, b, c, d, e, g, h, i):
    return (a, b, c, d, e, g, h, i)


print(f8(1, 2, 3, 4, 5, 6, 7, 8))
print(f8(8, 7, 6, 5, 4, 3, 2, 1))


# --- Defaults fill the correct registers ---
def fd(a, b=10, c=20):
    return (a, b, c)


print(fd(1))
print(fd(1, 2))
print(fd(1, c=99))
print(fd(1, b=2, c=3))


# --- Keyword-only and positional-only mix ---
def fmix(a, b, /, c, *, d, e=5):
    return (a, b, c, d, e)


print(fmix(1, 2, 3, d=4))
print(fmix(1, 2, c=3, d=4, e=6))


# --- A parameter captured as a cell var still binds correctly ---
def make_adder(step):
    def add(x):
        return x + step

    return add


add3 = make_adder(3)
print(add3(10), add3(20))


def make_counter(start):
    def bump():
        nonlocal start
        start += 1
        return start

    return bump


c = make_counter(100)
print(c(), c(), c())


# --- Self-reference register for recursion (precomputed self_bind) ---
def fact(n):
    if n <= 1:
        return 1
    return n * fact(n - 1)


print(fact(6))


# --- Mutual recursion (no self-bind, regular recursion) ---
def is_even(n):
    if n == 0:
        return True
    return is_odd(n - 1)


def is_odd(n):
    if n == 0:
        return False
    return is_even(n - 1)


print(is_even(10), is_odd(10))


# --- #1987: pure fn branching on type returns correct (uncached) results ---
def kind(x):
    return type(x).__name__


print(kind(1), kind(1.0), kind(1))
print(kind((1, 1)), kind((1, 1.0)))


# --- #1987: previously-memoized recursion still computes correctly ---
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


print([fib(i) for i in range(12)])


# --- *args / **kwargs binding ---
def fva(a, *args, **kw):
    return (a, args, sorted(kw.items()))


print(fva(1))
print(fva(1, 2, 3))
print(fva(1, 2, 3, x=9, y=8))


# --- Methods / classmethod / staticmethod argument binding ---
class C:
    def m(self, x, y):
        return ("m", x, y)

    @classmethod
    def cm(cls, x):
        return ("cm", cls.__name__, x)

    @staticmethod
    def sm(x, y):
        return ("sm", x, y)


o = C()
print(o.m(1, 2))
print(C.cm(3))
print(o.cm(3))
print(C.sm(4, 5))
print(o.sm(4, 5))
