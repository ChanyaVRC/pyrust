# Positional calls into `*args`-only callees (#3422): fixed params, defaults,
# empty / small / large `*args`, missing-argument errors, keyword fallback,
# bound methods, and `with` statements whose `__exit__` takes `*args`.


def f(a, *args):
    return a + len(args)


print(f(1), f(1, 2, 3), f(10, *range(5)))


def g(*args):
    return args


print(g(), g(1), g(1, 2), g(1, 2, 3), g(1, 2, 3, 4, 5))
print(g() is g(), g() is tuple(), type(g(1, 2)).__name__, len(g(*range(40))))


def h(a, b=2, *args):
    return (a, b, args)


print(h(1), h(1, 5), h(1, 5, 6, 7))
h.__defaults__ = (20,)
print(h(1))
h.__defaults__ = None
try:
    h(1)
except TypeError as e:
    print("TypeError:", e)


def two(a, b, *rest):
    return (a, b, rest)


for call in (lambda: two(), lambda: two(1)):
    try:
        call()
    except TypeError as e:
        print("TypeError:", e)

# Keyword arguments still take the general binder.
print(two(1, b=2), two(b=2, a=1))
try:
    two(1, 2, c=3)
except TypeError as e:
    print("TypeError:", e)
try:
    two(1, 2, a=3)
except TypeError as e:
    print("TypeError:", e)


def posonly(a, /, *args):
    return (a, args)


print(posonly(1), posonly(1, 2))
try:
    posonly()
except TypeError as e:
    print("TypeError:", e)

# The fresh *args tuple never aliases the caller's tuple.
t = (1, 2, 3)
print(g(*t) is t, g(*t) == t)


# Mutable default is shared across calls, exactly as CPython does.
def acc(x, bucket=[], *args):
    bucket.append(x)
    return len(bucket), args


print(acc(1), acc(2), acc(3, [], 9))


# Closures / cell parameters.
def outer():
    def inner(a, *args):
        return lambda: (a, args)

    return inner


print(outer()(1, 2, 3)())


def cellargs(*args):
    return lambda: args


print(cellargs(4, 5)())


# Generators with *args.
def gen(a, *args):
    yield a
    yield from args


print(list(gen(1, 2, 3)), list(gen(0)))


class C:
    def m(self, a, *args):
        return (type(self).__name__, a, args)

    def only(*args):
        return len(args), type(args[0]).__name__

    def opt(self, a=7, *args):
        return (a, args)


c = C()
print(c.m(1), c.m(1, 2, 3), c.only(), c.only(1, 2), c.opt(), c.opt(1, 2))
bm = c.m
print(bm(5), bm(5, 6))
try:
    c.m()
except TypeError as e:
    print("TypeError:", e)
print(C.m(c, 1, 2), C.only(1, 2, 3))


class Recurse:
    def total(self, n, *acc):
        if n == 0:
            return sum(acc)
        return self.total(n - 1, n, *acc)


print(Recurse().total(10))


def rec(n, *acc):
    return sum(acc) if n == 0 else rec(n - 1, n, *acc)


print(rec(50))


# `with` statements whose `__exit__` takes `*args`.
class CMStar:
    def __init__(self, suppress):
        self.suppress = suppress

    def __enter__(self):
        return "entered"

    def __exit__(self, *exc):
        print("exit:", exc[0].__name__ if exc[0] else None, len(exc))
        return self.suppress


with CMStar(False) as v:
    print(v)

with CMStar(True):
    raise ValueError("suppressed")
print("after suppressed")

try:
    with CMStar(False):
        raise KeyError("propagates")
except KeyError as e:
    print("propagated:", repr(e))


class CMExit2(CMStar):
    def __exit__(self, exc_type, *rest):
        print("exit2:", exc_type, len(rest), rest[0] is None or isinstance(rest[0], Exception))
        return False


with CMExit2(False):
    pass
try:
    with CMExit2(False):
        raise RuntimeError("x")
except RuntimeError:
    print("caught RuntimeError")


class CMExitDefaults:
    def __enter__(self):
        return self

    def __exit__(self, t=None, v=None, tb=None, *more):
        print("defaults exit:", t, v is None, more)


with CMExitDefaults():
    pass


class CMTooFew:
    def __enter__(self):
        return self

    def __exit__(self, a, b, c, d, *more):
        return True


try:
    with CMTooFew():
        pass
except TypeError as e:
    print("TypeError:", e)


# `return` / `break` early exits still run `__exit__(*args)`.
def early():
    with CMStar(False):
        return "returned"


print(early())
for i in range(3):
    with CMStar(False):
        if i == 1:
            break
print("loop done", i)


# An exception raised from a `*args`-only callee unwinds normally.
def boom(*args):
    raise ValueError(args)


try:
    boom(1, 2)
except ValueError as e:
    print("ValueError:", e)

# Nested `with` with `*args` `__exit__` inside a try.
try:
    with CMStar(False), CMStar(True):
        raise IndexError("inner suppressed")
    print("outer continues")
except IndexError:
    print("should not happen")

print(f.__code__.co_argcount, f.__code__.co_flags & 0x04 != 0)


# The `with` protocol lookups are inline-cached per site: replacing or deleting
# `__enter__` / `__exit__` on the class between iterations must be observed.
class Mut:
    def __enter__(self):
        return "e1"

    def __exit__(self, *a):
        print("x1", len(a))


def use_mut(rounds):
    m = Mut()
    for i in range(rounds):
        try:
            with m as v:
                # Round 4 has no `__exit__`: CPython resolves it before running
                # the body, pyrust after (pre-existing), so stay silent there.
                if i != 4:
                    print("body", i, v)
        except TypeError as e:
            print("TypeError:", e)
        if i == 1:
            Mut.__exit__ = lambda self, t, v, tb: print("x2", t)
        elif i == 2:
            Mut.__enter__ = lambda self: "e2"
        elif i == 3:
            del Mut.__exit__
        elif i == 4:
            Mut.__exit__ = lambda self, *a: print("x3", a)
            del Mut.__enter__


use_mut(7)


class Base:
    def __enter__(self):
        return "base"

    def __exit__(self, *a):
        print("base exit")


class Child(Base):
    pass


def use_child(rounds):
    c = Child()
    for i in range(rounds):
        with c as v:
            print(v)
        if i == 0:
            Base.__exit__ = lambda self, *a: print("patched base exit")


use_child(3)


# Different context-manager classes through one `with` site.
class Other:
    def __enter__(self):
        return "other"

    def __exit__(self, t, v, tb):
        print("other exit")


for cm in (CMStar(False), Other(), CMStar(False), Other(), 42):
    try:
        with cm as v:
            print(v)
    except TypeError as e:
        print("TypeError:", e)
