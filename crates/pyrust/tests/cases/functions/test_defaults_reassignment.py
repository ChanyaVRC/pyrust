# Reassigning `f.__defaults__` / `f.__kwdefaults__` must be observed by both the
# attribute reads and the call binder (#2395).  Before the fix the assignment was
# accepted but silently dropped, so calls kept using the compile-time defaults.
# CPython aligns the `__defaults__` tuple to the *last n* positional params at
# call time (the tuple length need not equal the parameter count) and keeps a
# separate `__kwdefaults__` dict for keyword-only params.


# --- basic positional reassignment, observed by keyword and positional calls ---
def dd(a, b=5):
    return (a, b)


dd.__defaults__ = (99,)
print(dd(a=1))  # (1, 99)
print(dd(1))  # (1, 99)
print(dd.__defaults__)  # (99,) — round-trips


# --- tuple aligns to the LAST n positional params ---
def f(a, b=2, /, c=3):
    return (a, b, c)


print(f.__defaults__)  # (2, 3) — positional-only defaults included
f.__defaults__ = (20, 30)
print(f(1))  # (1, 20, 30)
f.__defaults__ = (99,)
print(f(1, 2))  # (1, 2, 99) — only the last param gets the override


# --- empty tuple removes a default (getter returns (), not None) ---
def ee(a, b=5):
    return (a, b)


ee.__defaults__ = ()
print(ee.__defaults__)  # ()
try:
    ee(1)
except TypeError as e:
    print("TE:", e)  # missing 1 required positional argument: 'b'


# --- None clears all positional defaults ---
def gg(a, b=5):
    return (a, b)


gg.__defaults__ = None
print(gg.__defaults__)  # None
try:
    gg(1)
except TypeError as e:
    print("TE:", e)


# --- del resets to None, also clearing compile-time defaults ---
def hh(a, b=5):
    return (a, b)


del hh.__defaults__
print(hh.__defaults__)  # None
try:
    hh(1)
except TypeError as e:
    print("TE:", e)


# --- wrong type raises TypeError with CPython wording ---
def ww(a, b=5):
    return (a, b)


try:
    ww.__defaults__ = [1, 2]
except TypeError as e:
    print("WT:", e)  # __defaults__ must be set to a tuple object


# --- __kwdefaults__ is a separate dict for keyword-only params ---
def kk(a, *, b=5, c=6):
    return (a, b, c)


kk.__kwdefaults__ = {"b": 99}
print(kk.__kwdefaults__)  # {'b': 99}
try:
    kk(1)
except TypeError as e:
    print("KW-TE:", e)  # c lost its default → missing required keyword-only arg
print(kk(1, c=3))  # (1, 99, 3)
kk.__kwdefaults__ = None
print(kk.__kwdefaults__)  # None
try:
    kk.__kwdefaults__ = [1]
except TypeError as e:
    print("KW-WT:", e)  # __kwdefaults__ must be set to a dict object


# --- positional + keyword-only defaults overridden together ---
def both(a, b=1, *, c=2, d=3):
    return (a, b, c, d)


both.__defaults__ = (10,)
both.__kwdefaults__ = {"c": 20, "d": 30}
print(both(0))  # (0, 10, 20, 30)


# --- closures from one def keep independent defaults ---
def make():
    def inner(a, b=1):
        return (a, b)

    return inner


i1 = make()
i2 = make()
i1.__defaults__ = (100,)
print(i1(0), i2(0))  # (0, 100) (0, 1) — i2 is unaffected


# --- lambda ---
g = lambda a, b=1: (a, b)
g.__defaults__ = (7,)
print(g(0))  # (0, 7)


# --- method ---
class C:
    def m(self, a, b=1):
        return (a, b)


C.m.__defaults__ = (50,)
print(C().m(0))  # (0, 50)


# --- repeated reassignment at the same call site (cache-staleness probe) ---
def st(a, b=0):
    return (a, b)


print(st(1))  # (1, 0)
st.__defaults__ = (10,)
print(st(1))  # (1, 10)
st.__defaults__ = (20,)
print(st(1))  # (1, 20)
print(st(1, b=99))  # (1, 99) — keyword arg still overrides the override


# --- variadic functions observe the override too ---
def v(a, b=5, *args, **kw):
    return (a, b, args, kw)


v.__defaults__ = (99,)
print(v(1))  # (1, 99, (), {})
print(v(1, 2, 3))  # (1, 2, (3,), {})


# --- **splat call observes the override ---
def sp(a, b=5):
    return (a, b)


sp.__defaults__ = (77,)
print(sp(**{"a": 1}))  # (1, 77)
