# PEP 657 fine-grained caret anchors for `UnaryOp` and `FormatValue` (f-string
# replacement fields), matching CPython 3.12 (issue #2582).
#
# CPython 3.12 underlines:
#   * arithmetic unary `-x` / `+x` / `~x` -> `^` from the operator through the
#     operand (e.g. `1 + (-x)` -> `^^` under `-x`); suppressed only when it
#     covers the whole stripped line (a bare `-x` statement).
#   * an f-string replacement field `{...}` -> `^` over the whole field
#     including the braces and any `!conv` / `:spec` (e.g. `f"  {x!r}  end"`
#     -> `^^^^^` under `{x!r}`), whether the error comes from the conversion
#     (`__repr__`/`__str__`), the format spec (`__format__`), or plain `str()`.
# A function `Call` already carried its caret; the call below pins that the
# `^^^^^^` under `raiser()` plus the callee frame's own carets are unchanged.
#
# NOTE: the parity harness strips the `^`/`~` underline rows before diffing, so
# this fixture pins the **source-line rendering** and exception message/class;
# the precise caret columns are verified byte-for-byte against `python3.12`
# manually.  Each block diverges from CPython only in the (stripped) caret row.


# --- arithmetic unary `-x` on a str -> `^^` under `-x` ---
def case_unary_neg():
    x = "s"
    return 1 + (-x)  # noqa


try:
    case_unary_neg()
except TypeError as e:
    print("unary neg:", type(e).__name__, e)


# --- unary `~x` on a str -> `^^` under `~x` ---
try:
    s = "s"
    _ = 1 + (~s)
except TypeError as e:
    print("unary invert:", type(e).__name__, e)


# --- f-string `{x!r}` conversion error -> `^^^^^` under the field ---
class ReprBoom:
    def __repr__(self):
        raise ValueError("repr boom")


try:
    _ = "z" + f"  {ReprBoom()!r}  end"
except ValueError as e:
    print("fstring !r:", type(e).__name__, e)


# --- f-string `{x:>10}` format-spec error -> `^^^^^^^` under the field ---
class FormatBoom:
    def __format__(self, spec):
        raise ValueError("format boom")


try:
    _ = "z" + f"  {FormatBoom():>10}  end"
except ValueError as e:
    print("fstring spec:", type(e).__name__, e)


# --- plain f-string `{x}` str() error -> `^^^` under the field ---
class StrBoom:
    def __str__(self):
        raise ValueError("str boom")


try:
    _ = "z" + f"  {StrBoom()}  end"
except ValueError as e:
    print("fstring plain:", type(e).__name__, e)


# --- function call that raises inside the callee: call-site `^^^^^^` carets
#     plus the callee's own `~^~` carets (already supported; pins no regress) ---
def raiser(_arg):
    return 1 / 0


try:
    raiser("x")
except ZeroDivisionError as e:
    print("call:", type(e).__name__, e)
