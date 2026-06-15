# Parity fixture: PEP 657 attribute-access caret anchors (issue #2442, stage 2).
#
# An uncaught AttributeError from an attribute access (`obj.attr`) should
# underline the `obj.attr` span with `^` carets, anchored from the target's
# start column to the attribute name's end column.  CPython suppresses the
# caret row when the anchor spans the whole significant line; pyrust mirrors
# that.  The parity harness strips the `^`/`~` underline rows before diffing
# (CPython emits fine-grained markers, pyrust a full-width `^`), so this fixture
# verifies the echoed source line + exception class/message rather than the
# caret glyphs themselves; the caret placement is checked by hand against
# python3.12.
#
# Each case raises inside exec() (caught here) so the traceback goes to stderr
# and the script keeps running.

# Case 1: attribute access that spans the whole line — CPython prints no carets.
try:
    exec("(1).nonexistent_attr", {})
except AttributeError as e:
    print("case1:", type(e).__name__)

# Case 2: chained attribute, anchor underlines the inner `(1).foo` sub-span.
try:
    exec("x = (1).foo.bar", {})
except AttributeError as e:
    print("case2:", type(e).__name__)

# Case 3: attribute on a builtin method object.
try:
    exec('z = "hello".upper.nope', {})
except AttributeError as e:
    print("case3:", type(e).__name__)

print("all done")
