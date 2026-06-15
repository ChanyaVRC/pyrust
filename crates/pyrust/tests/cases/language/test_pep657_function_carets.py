# Parity fixture: PEP 657 caret anchors on *function* traceback frames
# (issue #2443, stage 2 of #2426).
#
# Stage 1 (#2440) plumbed the PEP 657 column anchor only for the `<module>`
# frame.  Stage 2 threads each function frame's raising-instruction column span
# into its traceback entry, so an error propagated through nested calls draws
# the narrow caret on every frame — including the *outer* frames whose call site
# is a sub-expression (`1 + inner()`, `[a()][0]`, `10 * b()`).
#
# The parity harness strips the `^`/`~` underline row before diffing (CPython
# emits fine-grained column markers; the rows are byte-verified against
# python3.12 in `tests/uncaught_traceback_source_render.rs`).  This fixture
# therefore pins the *frame chain* the unwinder records: it catches each error
# and walks `__traceback__` (no `traceback` module dependency), printing every
# frame's function name + line so a dropped or misattributed frame is caught.


def walk(exc):
    tb = exc.__traceback__
    while tb is not None:
        frame = tb.tb_frame
        print(frame.f_code.co_name, tb.tb_lineno)
        tb = tb.tb_next


# ── Case 1: single function frame, narrow NameError anchor ───────────────────
def g():
    return undefined_in_func


try:
    g()
except NameError as e:
    print("case1:", type(e).__name__)
    walk(e)


# ── Case 2: nested function frames ───────────────────────────────────────────
def outer():
    def inner():
        x = undefined_nested  # noqa: F841

    inner()


try:
    outer()
except NameError as e:
    print("case2:", type(e).__name__)
    walk(e)


# ── Case 3: outer frame call site is a sub-expression (narrow anchor) ────────
def raiser():
    raise ValueError("boom")


def consumer():
    x = 1 + raiser()  # noqa: F841
    return x


try:
    consumer()
except ValueError as e:
    print("case3:", type(e).__name__, str(e))
    walk(e)


# ── Case 4: three trampolined frames, each with its own call-site anchor ─────
def a():
    raise ValueError("deep")


def b():
    return [a()][0]


def c():
    return 10 * b()


try:
    c()
except ValueError as e:
    print("case4:", type(e).__name__, str(e))
    walk(e)


print("all done")
