# Regression for issue #2517: when an attribute access (or subscription /
# slicing) raises inside a user function and the call result is *discarded*
# (the function is called as a statement, not captured), the exception must
# still propagate.  Previously is_pure_expr classified `Expr::Attr` /
# `Expr::Index` / `Expr::Slice` as pure when their operands were pure, so a
# function like `def f(): return (1).foo` was deemed pure, compiled with
# CallMemo, and silently dead-store-eliminated when its result was unused —
# the AttributeError vanished and the program exited 0.  PR #2518 marked those
# expression kinds impure; this fixture pins the observable behaviour.


# 1. AttributeError from attribute access, result discarded.
def attr_fail():
    return (1).foo


try:
    attr_fail()  # result discarded — must still raise
    print("BUG: attr_fail swallowed")
except AttributeError as e:
    print("attr:", type(e).__name__, str(e))


# 2. TypeError raised inside a user __getattribute__, result discarded.
class RaisesOnGetattribute:
    def __getattribute__(self, name):
        raise TypeError("getattribute boom")


def getattribute_fail():
    return RaisesOnGetattribute().anything


try:
    getattribute_fail()
    print("BUG: getattribute_fail swallowed")
except TypeError as e:
    print("getattribute:", type(e).__name__, str(e))


# 3. ValueError raised inside a user __getattr__, result discarded.
class RaisesOnGetattr:
    def __getattr__(self, name):
        raise ValueError("getattr boom")


def getattr_fail():
    return RaisesOnGetattr().missing


try:
    getattr_fail()
    print("BUG: getattr_fail swallowed")
except ValueError as e:
    print("getattr:", type(e).__name__, str(e))


# 4. IndexError from subscription, result discarded.
def index_fail():
    return [1, 2, 3][99]


try:
    index_fail()
    print("BUG: index_fail swallowed")
except IndexError as e:
    print("index:", type(e).__name__, str(e))


# 5. KeyError from mapping subscription, result discarded.
def key_fail():
    return {}["missing"]


try:
    key_fail()
    print("BUG: key_fail swallowed")
except KeyError as e:
    print("key:", type(e).__name__, repr(e.args[0]))


# 6. Exception inside __init__ during construction, result discarded.
class FailsInInit:
    def __init__(self):
        raise RuntimeError("init fail")


def construct_fail():
    return FailsInInit()


try:
    construct_fail()
    print("BUG: construct_fail swallowed")
except RuntimeError as e:
    print("init:", type(e).__name__, str(e))


# 7. TypeError from slicing an unsubscriptable object, result discarded.
# Covers the Expr::Slice arm #2518 marked impure (distinct from Index above).
class NoSubscript:
    pass


def slice_fail():
    return NoSubscript()[1:2]


try:
    slice_fail()
    print("BUG: slice_fail swallowed")
except TypeError as e:
    print("slice:", type(e).__name__, str(e))


# 8. Still NOT swallowed when the result is captured (control case).
def attr_fail2():
    return [].nope


try:
    x = attr_fail2()
    print("BUG: captured swallowed")
except AttributeError as e:
    print("captured:", type(e).__name__, str(e))


print("done")
