# PEP 657 fine-grained caret anchors for `not` in a boolean-jump context,
# matching CPython 3.12 (issue #2588, follow-up to #2582/#2584).
#
# When `not expr` is the condition of an `if` / `while` (or any short-circuit
# branch), the compiler emits `UnaryOp(Not) + JumpIf*` and the optimizer's
# `pass_not_invert` collapses the pair into a single inverted conditional jump,
# dropping the `UnaryOp(Not)`.  The bool conversion (which fires the operand's
# `__bool__`) then happens on that jump.  CPython 3.12 anchors the jump at the
# **operand** span (not the whole `not operand`):
#
#   if not B():      ->  `^` under `B()`        (POP_JUMP_IF_TRUE @ operand)
#   while not B():   ->  `^` under `B()`
#
# By contrast a `not` left as a real `UnaryOp` (e.g. assigned, or the outer
# `not` of `not not B()`) keeps the whole `not operand` span (#2584):
#
#   x = not not B()  ->  `^^^^^^^` under the inner `not B()`
#
# NOTE: the parity harness strips the `^`/`~` underline rows before diffing, so
# this fixture pins the **source-line rendering** and exception message/class;
# the precise caret columns are verified byte-for-byte against `python3.12`
# manually.  Each block diverges from CPython only in the (stripped) caret row.


class B:
    def __bool__(self):
        raise ValueError("bool boom")


# --- `if not B():` -> `^` under `B()` (operand), jump-anchored ---
def case_if_not():
    if not B():
        return 1
    return 0


try:
    case_if_not()
except ValueError as e:
    print("if not:", type(e).__name__, e)


# --- `while not B():` -> `^` under `B()` (operand), jump-anchored ---
def case_while_not():
    while not B():
        return 1
    return 0


try:
    case_while_not()
except ValueError as e:
    print("while not:", type(e).__name__, e)


# --- `not not B()` -> `^^^^^^^` under inner `not B()` (UnaryOp, #2584) ---
def case_not_not():
    return not not B()


try:
    case_not_not()
except ValueError as e:
    print("not not:", type(e).__name__, e)
