# PEP 3134: implicit exception chaining via __context__
#
# Whenever a `raise` happens inside an `except` handler (or `finally`),
# CPython attaches the currently-handled exception as the new
# exception's `__context__`.  This file exercises the cases that
# pyrust historically missed (Issue #387).


# 1. Context is set when a new exception is raised inside an except handler.
try:
    try:
        raise ValueError("first")
    except ValueError:
        raise RuntimeError("second")
except RuntimeError as e:
    assert isinstance(e.__context__, ValueError)
    assert e.__context__.args[0] == "first"
    print("ctx-basic", type(e.__context__).__name__, e.__context__.args[0])


# 2. No context when raising outside any except handler.
try:
    raise ValueError("x")
except ValueError as e:
    assert e.__context__ is None
    print("ctx-none-outside", e.__context__ is None)


# 3. Nested context chain — __context__ of __context__ walks back.
class A(Exception):
    pass


class B(Exception):
    pass


class C(Exception):
    pass


try:
    try:
        try:
            raise A("a")
        except A:
            raise B("b")
    except B:
        raise C("c")
except C as e:
    assert isinstance(e.__context__, B)
    assert isinstance(e.__context__.__context__, A)
    inner_name = type(e.__context__).__name__
    outer_name = type(e.__context__.__context__).__name__
    print("ctx-chain", inner_name, outer_name)


# 4. After the outer try/except finishes, the handled-exception stack is
# clean: a fresh raise outside any handler has __context__ = None.
try:
    raise ValueError("after-outer")
except ValueError as e:
    assert e.__context__ is None
    print("ctx-clean-after", e.__context__ is None)


# 5. Function call: raise inside an except inside a function — when the
# uncaught exception propagates, the outer caller's handler observes the
# context (and the inner frame's stale handler-stack entries don't leak).
def raise_from_handler():
    try:
        raise ValueError("inside-fn")
    except ValueError:
        raise TypeError("bubbled")


try:
    raise_from_handler()
except TypeError as e:
    assert isinstance(e.__context__, ValueError)
    print("ctx-across-fn", type(e.__context__).__name__)


# After raise_from_handler returns via exception, a *new* try/except in
# the caller must NOT see leftover context from the inner frame.
try:
    raise ValueError("post-fn")
except ValueError as e:
    assert e.__context__ is None
    print("ctx-no-leak", e.__context__ is None)


# 6. `raise X from Y` still works: __cause__ set, __context__ also
# observable (CPython sets both; __suppress_context__ becomes True).
try:
    try:
        raise ValueError("cause-orig")
    except ValueError as inner:
        raise RuntimeError("with-cause") from inner
except RuntimeError as e:
    assert isinstance(e.__cause__, ValueError)
    assert isinstance(e.__context__, ValueError)
    print("ctx-with-from", type(e.__cause__).__name__, type(e.__context__).__name__)


# 7. Bare re-raise (`raise` inside except) does NOT create a self-cycle.
def reraise():
    try:
        raise RuntimeError("re")
    except RuntimeError:
        raise


# Bare re-raise re-raises the same instance; its __context__ must
# not point at itself.
try:
    reraise()
except RuntimeError as e:
    assert e.__context__ is None or e.__context__ is not e
    print("ctx-bare-reraise-no-cycle", True)


# 8. Early exit from handler via `return` does not leak context for a
# subsequent raise in the caller.
def early_return():
    try:
        raise ValueError("ret")
    except ValueError:
        return "ok"


val = early_return()
try:
    raise ValueError("after-return")
except ValueError as e:
    assert e.__context__ is None
    print("ctx-no-leak-return", val, e.__context__ is None)


# 9. Exception from an inner finally clause raised inside an outer except
# handler still picks up the outer's currently-handled exception.
class D(Exception):
    pass


try:
    try:
        raise A("a-fin")
    except A:
        try:
            pass
        finally:
            raise D("d-fin")
except D as e:
    assert isinstance(e.__context__, A)
    print("ctx-finally", type(e.__context__).__name__)


# 10. Exception raised inside a `finally:` clause picks up the original
# in-flight exception as context, even with no surrounding `except`.
try:
    try:
        raise A("a-only-finally")
    finally:
        raise D("d-from-finally")
except D as e:
    assert isinstance(e.__context__, A)
    print("ctx-finally-only", type(e.__context__).__name__)
