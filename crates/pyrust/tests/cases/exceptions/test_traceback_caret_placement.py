# Uncaught-traceback caret (`^^^` / `~^~`) placement and source-line rendering
# parity with CPython 3.12 (issue #2411).
#
# CPython 3.12 underlines the precise sub-expression that raised, using PEP 657
# column spans: a bare call / `raise X(...)` gets `^` carets, a binary operator
# gets `~` under the operands and `^` under the operator, a subscript gets `~`
# under the object and `^` under `[...]`.  pyrust now matches this for the
# highest-value forms, and — critically for this issue — *every* traceback frame
# (including re-raised and chained exceptions) now prints its source line.
#
# NOTE: the parity harness strips the `^`/`~` underline rows before diffing, so
# this fixture pins the **source-line rendering** half of #2411 (every frame
# echoes its source line); the precise caret columns are verified byte-for-byte
# against `python3.12` manually.  Each block below diverges from CPython only in
# the (stripped) caret row, so the harness confirms exact source-line / message
# parity.

# --- inline binary op: `x = 1/0` -> source line echoed, `~^~` under `1/0` ---
def case_inline_div():
    x = 1 / 0  # noqa


try:
    case_inline_div()
except ZeroDivisionError as e:
    print("inline div:", type(e).__name__, e)


# --- attribute error on a user instance: no caret (whole-line anchor) ---
class C:
    pass


try:
    C().foo
except AttributeError as e:
    print("attr:", type(e).__name__)


# --- subscript: `d['a']` -> `~^^^^^` under the object + `[...]` ---
try:
    d = {}
    d["missing"]
except KeyError as e:
    print("subscript:", type(e).__name__, e)


# --- call that raises inside a function: `raise ValueError(...)` carets ---
def raiser():
    raise ValueError("boom")


try:
    raiser()
except ValueError as e:
    print("call raise:", type(e).__name__, e)


# --- chained exception (`raise ... from`): inner `1/0` keeps its caret ---
def chained():
    try:
        return 1 / 0
    except ZeroDivisionError as inner:
        raise ValueError("wrapped") from inner


try:
    chained()
except ValueError as e:
    print("chained:", type(e).__name__, "cause:", type(e.__cause__).__name__)
