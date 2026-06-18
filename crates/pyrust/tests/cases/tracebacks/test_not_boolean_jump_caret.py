# PEP 657 fine-grained caret anchors for `not` in a boolean-jump context,
# matching CPython 3.12 (issue #2588, follow-up to #2582/#2584).
#
# When `not expr` is the condition of an `if` / `while` (or any short-circuit
# branch), the optimizer's `pass_not_invert` *can* collapse the
# `UnaryOp(Not) + JumpIf*` pair into a single inverted conditional jump,
# dropping the `UnaryOp(Not)`.  The bool conversion (which fires the operand's
# `__bool__`) then happens on that jump, and CPython 3.12 anchors the jump at
# the **operand** span, not the whole `not operand`:
#
#   if not B():      ->  `^` under `B()`        (POP_JUMP_IF_TRUE @ operand)
#   while not B():   ->  `^` under `B()`
#
# This fusion only happens at **module scope** in pyrust; inside a function the
# `not` is left as a real `UnaryOp` (so the caret spans the whole `not B()` and
# diverges from CPython's operand-only anchor).  PR #2592 (issue #2588) is what
# fixes the *fused* module-scope path to recover the operand caret; the
# function-scope blocks below are kept for the contrasting (non-fused) form.
#
# A `not` left as a real `UnaryOp` (e.g. the outer `not` of `not not B()`)
# keeps the whole `not operand` span (#2584):
#
#   x = not not B()  ->  `^^^^^^^` under the inner `not B()`
#
# NOTE: the parity harness strips the `^`/`~` underline rows before diffing, so
# this fixture pins the **source-line rendering** and exception message/class;
# the precise caret columns are verified byte-for-byte against `python3.12`
# manually.  The module-scope blocks below are the ones that exercise the
# actual fused-jump path PR #2592 fixed.


class B:
    def __bool__(self):
        raise ValueError("bool boom")


# === module scope: `not` fuses into the boolean-jump (the PR #2592 fix) ===

# --- `if not B():` -> `^` under `B()` (operand), jump-anchored ---
try:
    if not B():
        pass
except ValueError as e:
    print("module if not:", type(e).__name__, e)


# --- `while not B():` -> `^` under `B()` (operand), jump-anchored ---
try:
    while not B():
        break
except ValueError as e:
    print("module while not:", type(e).__name__, e)


# === function scope: `not` stays a UnaryOp (non-fused, contrasting form) ===

# --- `if not B():` ---
def case_if_not():
    if not B():
        return 1
    return 0


try:
    case_if_not()
except ValueError as e:
    print("if not:", type(e).__name__, e)


# --- `while not B():` ---
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
