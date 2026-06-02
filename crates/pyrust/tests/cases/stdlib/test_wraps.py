# functools.wraps / functools.update_wrapper — full attribute copy.
#
# Issues #2017 / #2063: `@wraps(orig)` must mutate and return the wrapper
# *function* (not a synthetic object), copying WRAPPER_ASSIGNMENTS
# (`__module__`, `__name__`, `__qualname__`, `__annotations__`,
# `__doc__`; skipping any the wrapped object lacks), merging `__dict__`,
# and setting `__wrapped__`.

import functools


def orig(a: int, b: str) -> bool:
    "the doc"
    return a


orig.custom = "X"
orig.other = 42


@functools.wraps(orig)
def w(*a, **k):
    return orig(*a, **k)


# Result is a real function, not a synthetic wrapper object.
print("type", type(w).__name__)
print("name", w.__name__)
print("qualname", w.__qualname__)
print("doc", w.__doc__)
print("module", w.__module__)
print("annotations", w.__annotations__)
print("wrapped-is-orig", w.__wrapped__ is orig)
# __dict__ merged from the wrapped function.
print("custom", w.custom)
print("other", w.other)
print("dict-keys", sorted(w.__dict__.keys()))
# Wrapper still callable, delegating to the original.
print("call", w(1, 2))


# __dict__ merge precedence: wrapped wins on conflict; wrapper-only keys
# are retained.
def base():
    pass


base.shared = "from_base"
base.only_base = "b"


def wrapper2():
    pass


wrapper2.shared = "from_wrapper"
wrapper2.only_wrapper = "w"
wrapper2 = functools.wraps(base)(wrapper2)
print("merge", wrapper2.shared, wrapper2.only_base, wrapper2.only_wrapper)


# Wrapping an object that lacks __name__ (a partial) skips that attr, so
# the wrapper keeps its own name.
p = functools.partial(len)


def wp(*a):
    return None


wp = functools.wraps(p)(wp)
print("partial-name", wp.__name__)
print("partial-wrapped-is-p", wp.__wrapped__ is p)


# Wrapping a function with no docstring → __doc__ copied as None.
def nodoc(x):
    return x


@functools.wraps(nodoc)
def wn(*a):
    return None


print("nodoc-doc", repr(wn.__doc__))


# update_wrapper called directly returns the (mutated) wrapper.
def src():
    "src doc"
    return 1


def dst():
    return 2


returned = functools.update_wrapper(dst, src)
print("update-returns-wrapper", returned is dst)
print("update-name", dst.__name__, "update-doc", dst.__doc__)


# Wrapping a bound method copies its metadata.
class A:
    def meth(self, x):
        "method doc"
        return x


a = A()


@functools.wraps(a.meth)
def wm(*args, **kw):
    return None


print("method-name", wm.__name__, "method-doc", repr(wm.__doc__))


# Wrapping a built-in (whose missing attrs surface as AttributeError on a
# different error path) must skip them, not raise.  CPython's `len` has no
# __doc__-less / __annotations__ attribute exposed the same way pyrust does;
# the copy should silently skip whatever is missing.
def wl(*a):
    return None


wl = functools.wraps(len)(wl)
print("builtin-name", wl.__name__, "builtin-qualname", wl.__qualname__)
print("builtin-wrapped-is-len", wl.__wrapped__ is len)


# __wrapped__ chain survives nested @wraps.
def f0():
    pass


@functools.wraps(f0)
def f1():
    pass


@functools.wraps(f1)
def f2():
    pass


print("chain", f2.__wrapped__ is f1, f1.__wrapped__ is f0)
