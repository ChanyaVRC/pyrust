# Regression test for issue #2632.
#
# When an exception is raised on a sub-expression that sits on a *continuation*
# line of a multi-line statement, the traceback must report the line that the
# failing sub-expression is on — not the statement's first physical line.
#
# CPython 3.12 gives each name node its own `lineno`; pyrust historically stamped
# every instruction in a statement with the statement's first line, so a name
# load on a later line reported the wrong line (and source text). The fix stamps
# the name-load instruction with the name's own line.
#
# We assert the reported line via `sys.exc_info()[2].tb_lineno`, which is the same
# value the stderr traceback formatter uses to pick the displayed source line.
# Source lines are fixed in this file, so the expected numbers are stable across
# CPython 3.11 / 3.12. The harness diffs pyrust's stdout against CPython 3.12's.
import sys


def reported_line():
    return sys.exc_info()[2].tb_lineno


# --- multi-line name lookup: the name is on the SECOND line ---
try:
    val = (1 +
        missing_op)
except NameError:
    print("multiline name line:", reported_line())  # 26

# --- name on the FIRST line of a multi-line expression keeps line 1 ---
try:
    val2 = (missing_first +
        1)
except NameError:
    print("multiline first-line name:", reported_line())  # 32

# --- three-line expression, failing name on the THIRD line ---
try:
    result = (
        1 +
        undefined_var
    )
except NameError:
    print("three-line name:", reported_line())  # 41

# --- single-line bare name must not regress ---
try:
    undefined_single
except NameError:
    print("single-line name:", reported_line())  # 48

# --- single-line assignment on a later physical line ---
x = 1
try:
    y = undefined_assign
except NameError:
    print("single-line assign:", reported_line())  # 55

# --- the operator (not a name) raising on a multi-line expr keeps its line ---
# The left operand "a" is loaded and the BinOp raises TypeError; CPython anchors
# the operator to the expression's first line.
try:
    z = ("a" +
        5)
except TypeError:
    print("multiline op line:", reported_line())  # 63

# --- the FIRST of two undefined names raises, on line 1 ---
try:
    w = (undef_one +
        undef_two)
except NameError as e:
    print("first-undef name:", reported_line())  # 70
    print("first-undef which:", str(e))          # name 'undef_one' ...
