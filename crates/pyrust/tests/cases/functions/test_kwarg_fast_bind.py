# Parity fixture for #2382 — the CallKw fast-bind path for keyword-argument
# calls.  A per-call-site cache maps keyword names to parameter slots so a plain
# keyword call binds without the per-call name scan; the general binder still
# owns every CPython-parity diagnostic.  Exercise the full binding matrix so any
# wrong slot, wrong fallback, or wrong error wording diverges from CPython.


def show(thunk):
    try:
        print("OK", repr(thunk()))
    except Exception as e:
        print(type(e).__name__, str(e))


def f(a, b, c):
    return (a, b, c)


def g(a, b=2, c=3):
    return (a, b, c)


def po(x, y, /, z):
    return (x, y, z)


def ko(a, *, k, m=9):
    return (a, k, m)


def kw(a, b, **rest):
    return (a, b, sorted(rest.items()))


# --- Happy paths: full-kw, mixed, defaults, keyword-only, posonly, **kwargs ---
show(lambda: f(a=1, b=2, c=3))
show(lambda: f(1, b=2, c=3))
show(lambda: f(1, 2, c=3))
show(lambda: f(c=3, a=1, b=2))
show(lambda: g(a=1))
show(lambda: g(a=1, c=30))
show(lambda: g(1, c=30))
show(lambda: ko(1, k=5))
show(lambda: ko(1, k=5, m=6))
show(lambda: ko(a=1, k=2, m=3))
show(lambda: kw(a=1, b=2))
show(lambda: kw(1, b=2, x=9, y=10))
show(lambda: po(1, 2, z=3))


# --- Error paths: wording must match CPython 3.12 byte-for-byte ---
show(lambda: f(a=1, b=2, d=3))   # unexpected keyword
show(lambda: f(1, a=2, c=3))     # multiple values for argument 'a'
show(lambda: f(a=1, b=2))        # missing required positional 'c'
show(lambda: g())                # missing required positional 'a'
show(lambda: po(1, 2, x=3))      # positional-only passed as keyword (+ missing z)
show(lambda: po(x=1, y=2, z=3))  # both positional-only as keyword
show(lambda: ko(1))              # missing keyword-only 'k'
show(lambda: ko(1, k=5, z=9))    # unexpected keyword 'z'
show(lambda: f(1, 2, 3, d=4))    # too many positional + unexpected keyword


# --- Closures sharing one `def` (same params, distinct identities) at one
#     call site: the cache keys on the shared param_binds, so both bind right.
def make(n):
    def inner(a, b, c):
        return a + b + c + n

    return inner


f1 = make(100)
f2 = make(200)


def call(fn):
    return fn(a=1, b=2, c=3)


print([call(f1 if i % 2 == 0 else f2) for i in range(6)])


# --- Polymorphic call site: different functions (different params) at one
#     source site — the param_binds guard must prevent a misbind.
def p3(a, b, c):
    return ("p3", a, b, c)


def q3(x, y, z):
    return ("q3", x, y, z)


def go(fn):
    try:
        return fn(a=1, b=2, c=3)
    except TypeError as e:
        return ("ERR", str(e))


for fn in (p3, q3, p3, q3):
    print(go(fn))


# --- Steady-state hits with defaults filled per call. ---
def steady(a, b=10, c=20):
    return a * 100 + b * 10 + c


print([steady(a=i, c=i) for i in range(4)])


# --- Generators, coroutines, lambdas, super(), all called with keywords. ---
def gen(a, b, c):
    yield a
    yield b
    yield c


print(list(gen(a=1, b=2, c=3)))
print(list(gen(1, c=3, b=2)))

lam = lambda a, b, c=0: (a, b, c)
print(lam(a=5, b=6))
print(lam(b=6, a=5, c=7))


class Base:
    def __init__(self, a, b):
        self.s = a + b


class Sub(Base):
    def __init__(self, a, b):
        super().__init__(a=a, b=b)


print(Sub(3, 4).s)


# --- Methods invoked with keywords (these take the variadic path, not CallKw,
#     but must stay correct alongside the new opcode). ---
class C:
    def m(self, a, b, c=0):
        return (a, b, c)


o = C()
print(o.m(a=1, b=2))
print(o.m(1, b=2, c=3))


# --- Default object identity isn't shared/leaked across calls. ---
def acc(x, store=None):
    if store is None:
        store = []
    store.append(x)
    return store


print(acc(x=1))
print(acc(x=2))
