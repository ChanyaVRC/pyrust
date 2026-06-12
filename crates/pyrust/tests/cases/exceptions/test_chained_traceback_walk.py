# Issues #2408/#2412: the uncaught stderr printer renders each chained
# exception's OWN traceback block (header + File frames + class line) above the
# connecting banner.  That rendering walks the chained exception's
# `__context__` / `__cause__` and its `__traceback__` chain — this fixture
# asserts that underlying machinery (the part the parity harness can observe
# without letting the exception escape) matches CPython 3.12.
#
# Asserts STRUCTURE (frame names + line numbers, banner-selecting flags) only —
# never object addresses or caret/source-line rendering (those diverge: #2411).


def walk(tb):
    rows = []
    node = tb
    while node is not None:
        rows.append((node.tb_lineno, node.tb_frame.f_code.co_name))
        node = node.tb_next
    return rows


# --- implicit __context__ chain (raise during handling) ---
def f_ctx():
    raise IndexError("idx")


def g_ctx():
    try:
        f_ctx()
    except IndexError:
        raise ValueError("v")


try:
    g_ctx()
except ValueError as e:
    ctx = e.__context__
    print("context class:", type(ctx).__name__)
    print("context suppress:", e.__suppress_context__)
    print("context cause:", e.__cause__)
    print("context frames:", walk(ctx.__traceback__))


# --- explicit __cause__ chain (raise X from Y) ---
def f_cause():
    raise IndexError("idx")


def g_cause():
    try:
        f_cause()
    except IndexError as inner:
        raise ValueError("v") from inner


try:
    g_cause()
except ValueError as e:
    print("cause class:", type(e.__cause__).__name__)
    print("cause suppress:", e.__suppress_context__)
    print("cause frames:", walk(e.__cause__.__traceback__))


# --- raise X from None suppresses the context ---
try:
    try:
        raise IndexError("idx")
    except IndexError:
        raise ValueError("v") from None
except ValueError as e:
    print("from-None cause:", e.__cause__)
    print("from-None suppress:", e.__suppress_context__)
    # __context__ is still recorded, but __suppress_context__ hides it.
    print("from-None context class:", type(e.__context__).__name__)


# --- chained exception constructed but never raised has no traceback ---
ctx_never = IndexError("never raised")
try:
    try:
        raise RuntimeError("real")
    except RuntimeError as e:
        e.__context__ = ctx_never
        raise
except RuntimeError as e:
    print("never-raised tb:", e.__context__.__traceback__)


# --- three-deep chain: each link carries its own context ---
def a3():
    raise IndexError("first")


def b3():
    try:
        a3()
    except IndexError:
        raise KeyError("second")


def c3():
    try:
        b3()
    except KeyError:
        raise ValueError("third")


try:
    c3()
except ValueError as e:
    mid = e.__context__
    print("3-deep mid class:", type(mid).__name__)
    print("3-deep oldest class:", type(mid.__context__).__name__)
    # Issue #2407 (fixed): a fresh raise inside a handler resets the stale
    # captured-frame snapshot, so each link's traceback walk carries ONLY its
    # own unwind frames — no spurious inner frame from the link it was handling.
    print("3-deep outer own walk:", walk(e.__traceback__))
    print("3-deep mid walk:", walk(mid.__traceback__))
    print("3-deep oldest walk:", walk(mid.__context__.__traceback__))
