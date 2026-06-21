# Issue #2728: iterating a mappingproxy must raise
# `RuntimeError: dictionary changed size during iteration` when the underlying
# mapping changes size mid-iteration, in all four combinations:
# forward/reverse x class-backed (vars(C)) / dict-backed (d.keys().mapping).


def show(it, mutate, show_key=True):
    # Class-backed proxies expose synthetic attrs (`__doc__` / `__weakref__` /
    # `__dict__`) whose presence/order is implementation-specific, so those cases
    # pass `show_key=False` and assert only the guard outcome, keeping the fixture
    # stable across CPython builds.
    key = next(it)
    if show_key:
        print(key)
    mutate()
    try:
        next(it)
        print("no error")
    except RuntimeError as e:
        print(e)


# --- Forward, dict-backed ---
d = {"a": 1, "b": 2, "c": 3}
show(iter(d.keys().mapping), lambda: d.__setitem__("x", 99))


# --- Forward, class-backed ---
class Foo:
    a = 1
    b = 2
    c = 3


def add_foo():
    Foo.dd = 4


show(iter(vars(Foo)), add_foo, show_key=False)


# --- Forward via for-loop, class-backed ---
class Bar:
    a = 1
    b = 2
    c = 3


seen = []
try:
    for k in vars(Bar):
        seen.append(k)
        if len(seen) == 1:
            Bar.zz = 9
    print("loop no error")
except RuntimeError as e:
    print("loop", e)


# --- Reverse, dict-backed ---
d2 = {"a": 1, "b": 2, "c": 3}
show(reversed(d2.keys().mapping), lambda: d2.__setitem__("x", 99))


# --- Reverse, class-backed ---
class Baz:
    a = 1
    b = 2
    c = 3


def add_baz():
    Baz.qq = 4


show(reversed(vars(Baz)), add_baz, show_key=False)


# --- Direct __reversed__() dunder, dict-backed ---
d3 = {"a": 1, "b": 2, "c": 3}
show(d3.keys().mapping.__reversed__(), lambda: d3.__setitem__("x", 99))


# --- __reversed__() rejects positional arguments ---
try:
    d3.keys().mapping.__reversed__(1)
except TypeError as e:
    print(e)


# --- Clean exhaustion then mutate: stays StopIteration, never RuntimeError ---
d4 = {"a": 1}
it = iter(d4.keys().mapping)
print(next(it))
try:
    next(it)
except StopIteration:
    print("stop")
d4["x"] = 2
try:
    next(it)
    print("clean")
except StopIteration:
    print("stop again")
except RuntimeError as e:
    print("ERR", e)


# --- No mutation: full forward + reverse walk completes ---
d5 = {"a": 1, "b": 2}
print(list(iter(d5.keys().mapping)))
print(list(reversed(d5.keys().mapping)))
