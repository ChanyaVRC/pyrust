# Parity fixture for #2329 / #2394 / #2430: call-binding TypeError messages
# use the function's __qualname__ (e.g. "C.m") rather than the bare __name__
# ("m"), matching CPython 3.12.


def show(fn):
    try:
        fn()
    except TypeError as e:
        print(e)


class C:
    def m(self, x):
        pass


# #2329: too many positional arguments.
show(lambda: C().m(1, 2))  # C.m() takes 2 positional arguments but 3 were given

# #2394: same param given positionally and by keyword.
show(lambda: C().m(1, x=2))  # C.m() got multiple values for argument 'x'

# #2430: unbound method call missing required positionals.
show(lambda: C.m())  # C.m() missing 2 required positional arguments: 'self' and 'x'

# Unexpected keyword argument.
show(lambda: C().m(y=5))  # C.m() got an unexpected keyword argument 'y'


# Plain module-level function: __qualname__ == __name__, so bare name.
def f(x):
    pass


show(lambda: f(1, 2))  # f() takes 1 positional argument but 2 were given


# Nested function: qualname carries the "<locals>" prefix.
def outer():
    def inner(a):
        pass

    return inner


show(lambda: outer()(1, 2))  # outer.<locals>.inner() takes 1 positional ...


# Variadic path: missing keyword-only argument through a *args function.
class D:
    def k(self, *args, y):
        pass


show(lambda: D().k())  # D.k() missing 1 required keyword-only argument: 'y'


# Positional-only param passed by keyword.
class E:
    def g(self, a, b, /):
        pass


show(
    lambda: E().g(a=1, b=2)
)  # E.g() got some positional-only arguments passed as keyword arguments: 'a, b'


# "from N to M" arity form (some params have defaults).
class F:
    def h(self, a, b=1):
        pass


show(lambda: F().h(1, 2, 3))  # F.h() takes from 2 to 3 positional arguments but 4 ...


# A user-reassigned __qualname__ is reflected in the message (CPython 3.12).
def renamed(x):
    pass


renamed.__qualname__ = "Renamed"
show(lambda: renamed(1, 2))  # Renamed() takes 1 positional argument but 2 were given
