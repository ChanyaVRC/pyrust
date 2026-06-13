# Regression test for issue #2439.
#
# A bare expression statement (one whose result is discarded) that raises must
# report ITS OWN source line in the traceback, not the previous statement's line.
# The defect is specific to the MODULE frame: each `def` is its own FnCode with an
# unambiguous line table, but the module body mixes many statements whose
# optimized instructions are matched back to the original stream by structural
# equality.  A register-renumbering pass (copy-prop / const-reg-prop) rewrites the
# operands of the raising instruction (the `GetAttr`/`GetItem`/`BinOp` behind a
# bare `obj.attr` / `obj[k]` / `a / b`), so it no longer matches its origin
# structurally and the greedy line-remap scan fell back to the previous
# statement's line.  The fix anchors such a side-effecting opcode to the nearest
# same-opcode origin, recovering its line.
#
# Verified at module scope by printing the absolute reported line of each bare
# raising statement.  The harness diffs pyrust's stdout against CPython 3.12's;
# the source lines are fixed in this file, so the expected numbers are stable.
# The `_x = _obj` copy is essential: it forces copy-prop to renumber the bare
# statement's operand, which is exactly what desynced the line-remap scan.
import sys


def reported_line():
    return sys.exc_info()[2].tb_lineno


_obj = object()
_x = _obj
try:
    _x.foo
except AttributeError:
    print("bare attr line:", reported_line())

_d = {"k": 1}
_y = _d
try:
    _y["missing"]
except KeyError:
    print("bare subscript line:", reported_line())

_a = 10**400
_b = _a
try:
    _b / 2
except OverflowError:
    print("bare binop line:", reported_line())

# Two structurally-identical bare attribute statements must report their own
# distinct lines (no cross-attribution).
_p = object()
_q = _p
try:
    _q.alpha
except AttributeError:
    line_a = reported_line()
try:
    _q.beta
except AttributeError:
    line_b = reported_line()

print("bare attr A line:", line_a)
print("bare attr B line:", line_b)
print("bare attrs differ:", line_a != line_b)
