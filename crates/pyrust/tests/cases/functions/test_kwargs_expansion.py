# Parity fixture for #2393 — the CallEx fast bind for double-splat expansion
# calls `f(<pos…>, **d)`.  A per-call-site shape cache keyed on (callee identity,
# npos, dict key-set) binds the splat dict's values straight into parameter slots
# (reusing the #2382 fast bind) with no dict copy and no name scan; the general
# binder still owns every CPython-parity diagnostic.  Exercise the binding matrix,
# polymorphic shapes, dict mutation, alternate mappings, and the error paths so
# any wrong slot, wrong shape guard, or wrong error wording diverges from CPython.

from collections import OrderedDict


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


def absorb(a, **kw):
    return (a, sorted(kw.items()))


def allkw(**kw):
    return sorted(kw.items())


# --- happy path: pure **d ----------------------------------------------------
show(lambda: f(**{"a": 1, "b": 2, "c": 3}))
show(lambda: f(**{"c": 3, "a": 1, "b": 2}))  # any key order binds by name
show(lambda: g(**{"a": 1}))  # defaults fill b, c
show(lambda: g(**{"a": 1, "c": 9}))
show(lambda: ko(**{"a": 1, "k": 2}))
show(lambda: ko(**{"a": 1, "k": 2, "m": 3}))

# --- mixed positional + **d --------------------------------------------------
show(lambda: f(1, **{"b": 2, "c": 3}))
show(lambda: f(1, 2, **{"c": 3}))
show(lambda: g(1, **{"c": 9}))
show(lambda: ko(1, **{"k": 2}))

# --- **kwargs receiver (slow-path fallback, no slot bind) --------------------
show(lambda: absorb(1, **{"x": 9, "y": 8}))
show(lambda: allkw(**{"z": 1, "q": 2}))

# --- empty splat -------------------------------------------------------------
show(lambda: f(1, 2, 3, **{}))
show(lambda: allkw(**{}))

# --- ordering into a **kwargs receiver (insertion order preserved) -----------
def order(**kw):
    return list(kw)


show(lambda: order(**{"z": 1, "a": 2, "m": 3}))

# --- polymorphic shapes at one call site (shape cache re-resolves) -----------
def poly(d):
    return f(**d)


for d in (
    {"a": 1, "b": 2, "c": 3},
    {"c": 30, "a": 10, "b": 20},
    {"a": 100, "b": 200, "c": 300},
    {"b": 2, "c": 3, "a": 1},
):
    print("poly", poly(d))

# --- dict mutation: keys stable, values change → cache stays a hit -----------
d = {"a": 0, "b": 2, "c": 3}
acc = []
for i in range(4):
    d["a"] = i
    acc.append(f(**d))
print("mutate", acc)

# --- alternate mappings as **d ----------------------------------------------
od = OrderedDict([("a", 1), ("b", 2), ("c", 3)])
show(lambda: f(**od))
show(lambda: allkw(**od))

# --- error paths (general binder owns the wording) ---------------------------
show(lambda: f(**{"a": 1, "b": 2, "c": 3, "d": 4}))  # unexpected keyword
show(lambda: f(**{"a": 1, "b": 2}))  # missing required
show(lambda: f(1, **{"a": 9, "b": 2, "c": 3}))  # multiple values (pos vs **d)
show(lambda: po(1, 2, **{"x": 99}))  # positional-only as keyword
show(lambda: ko(1, **{}))  # missing keyword-only
show(lambda: allkw(**{1: 2}))  # non-str key → keywords must be strings

# --- closures from one def share the cached prototype ------------------------
def make():
    def inner(a, b, c):
        return a * 100 + b * 10 + c

    return inner


i1 = make()
i2 = make()
print("closures", i1(**{"a": 1, "b": 2, "c": 3}), i2(**{"a": 4, "b": 5, "c": 6}))
