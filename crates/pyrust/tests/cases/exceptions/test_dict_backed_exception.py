# Issue #2637: when an exception instance's `__dict__` is replaced wholesale
# (`exc.__dict__ = d`), the exception-machinery sites that read the carried
# dunders (`__traceback__` / `__cause__` / `__context__`) must route through the
# live dict, not the raw `entries` slot map.  `sys.exc_info()[2]` was the site
# that still used a raw `get` and handed back `None` for a dict-backed exception.
import sys


class MyError(Exception):
    pass


# sys.exc_info()[2] returns the live traceback for a dict-backed exception.
e = MyError("oops")
e.__dict__ = {"extra": 1}
try:
    raise e
except MyError:
    print(sys.exc_info()[2] is not None)  # True
    print(e.__traceback__ is not None)  # True
    print(e.extra)  # 1 (custom dict key still attribute-accessible)


# `raise ... from ...` preserves `__cause__` on a dict-backed exception.
try:
    raise ValueError("inner")
except ValueError:
    e2 = RuntimeError("outer")
    e2.__dict__ = {"note": "x"}
    try:
        raise e2 from ValueError("inner2")
    except RuntimeError as caught2:
        print(caught2.__cause__ is not None)  # True
        print(str(caught2.__cause__))  # inner2
        print(caught2.__suppress_context__)  # True


# Implicit chaining records `__context__` on a dict-backed exception.
try:
    raise ValueError("first")
except ValueError:
    e3 = RuntimeError("second")
    e3.__dict__ = {"k": 1}
    try:
        raise e3
    except RuntimeError as caught3:
        print(caught3.__context__ is not None)  # True
        print(str(caught3.__context__))  # first


# ExceptionGroup.subgroup copies metadata off a dict-backed source group.
eg = ExceptionGroup("grp", [ValueError("a"), TypeError("b")])
eg.__dict__ = {"tag": 1}
sub = eg.subgroup(ValueError)
print(sub.message)  # grp
print([type(x).__name__ for x in sub.exceptions])  # ['ValueError']


# `__notes__` is an ordinary `__dict__` attribute (not a C-level slot), so a
# `__dict__` swap drops a note added *before* it; the derived group must not
# inherit the stale pre-swap note.
eg2 = ExceptionGroup("g2", [ValueError("a")])
eg2.add_note("pre-swap")
eg2.__dict__ = {"tag": 1}
match2, _rest2 = eg2.split(ValueError)
print(getattr(match2, "__notes__", "NONE"))  # NONE


# `add_note` after a `__dict__` swap writes through to the live dict, so the
# note is visible and propagates to a derived group.
eg3 = ExceptionGroup("g3", [ValueError("a")])
eg3.__dict__ = {"tag": 1}
eg3.add_note("post-swap")
print(eg3.__notes__)  # ['post-swap']
sub3 = eg3.subgroup(ValueError)
print(getattr(sub3, "__notes__", "NONE"))  # ['post-swap']
