# A key present in two keyword sources of a call (`**d1, **d2`, `kw=v, **{kw:v}`,
# `**{kw:v}, kw=v`) is a TypeError in CPython (DICT_MERGE), not a silent
# overwrite.  The error uses the callee's `<module>.<qualname>` (issue #2413).
# ASCII-only output (Windows CI uses cp1252).


def show(fn):
    try:
        print(fn())
    except TypeError as e:
        print("TypeError:", e)


def f(a, b, c=0):
    return (a, b, c)


# Duplicate across two ** splats.
show(lambda: f(**{"a": 1, "b": 2}, **{"b": 9, "c": 3}))

# Named arg then a colliding ** splat.
show(lambda: f(a=1, **{"a": 2}, b=3))

# ** splat then a colliding named arg.
show(lambda: f(**{"a": 1}, a=2, b=3))

# Three splats, collision only in the third.
show(lambda: f(**{"a": 1}, **{"b": 2}, **{"a": 9, "c": 3}))

# Non-overlapping splats still bind fine.
show(lambda: f(**{"a": 1}, **{"b": 2}))
show(lambda: f(**{"a": 1}, **{"b": 2}, **{"c": 3}))

# Empty splat mixed with named args is fine.
show(lambda: f(**{}, a=1, b=2))

# A positional arg already bound, then a ** splat naming it, is the *binder's*
# "got multiple values for argument" (a distinct CPython error path).
show(lambda: f(1, **{"a": 2}))

# **kwargs catch-all: duplicate across splats still raises.
def g(**kw):
    return kw


show(lambda: g(**{"x": 1}, **{"y": 2}))
show(lambda: g(**{"x": 1}, **{"x": 2}))


# Method call: the error uses the class-qualified method name.
class C:
    def m(self, x, y=0):
        return (x, y)


c = C()
show(lambda: c.m(x=1, **{"x": 2}))
show(lambda: c.m(**{"x": 1}, **{"x": 9}))
show(lambda: c.m(**{"x": 1}, **{"y": 2}))
