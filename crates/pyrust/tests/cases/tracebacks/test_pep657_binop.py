# PEP 657 caret placement for *multi-line* binary-op expressions (issue #2571).
#
# When a binary-op expression straddles physical lines, CPython 3.12 echoes the
# expression's first (start) source line and underlines from the expression
# start to the end of that line with solid `^` carets.  pyrust previously
# emitted a nonsensical single-line span (mixing the left operand's column with
# the operator / right-operand columns from a later line), which the formatter
# dropped — so the caret row was missing entirely.  It now clamps the underline
# to the displayed line, matching CPython.
#
# NOTE: the parity harness strips the `^`/`~` underline rows before diffing, so
# this fixture pins the **source-line / line-number / message** parity for these
# multi-line forms (which previously could still echo the wrong line).  The
# precise caret columns were verified byte-for-byte against `python3.12`
# manually; see the issue / PR for the captured comparisons.

# --- operator at end of first line: caret = `^^^` under `1 +` ---
try:
    x = (1 +
         "s")
except TypeError as e:
    print("op-trailing:", type(e).__name__, e)


# --- operand alone on first line, operator on its own line ---
try:
    y = (1
         +
         "s")
except TypeError as e:
    print("op-own-line:", type(e).__name__, e)


# --- multi-line multiplication ---
try:
    z = ([1] *
         "x")
except TypeError as e:
    print("mul:", type(e).__name__, e)


# --- multi-line power ---
try:
    w = (2 **
         "p")
except TypeError as e:
    print("pow:", type(e).__name__, e)


# --- three-line subtraction chain ---
try:
    q = (10
         - "a"
         - 3)
except TypeError as e:
    print("sub:", type(e).__name__, e)


# --- single-line binary op still gets its fine-grained `~~^~~` (no regression) ---
try:
    s = 1 + "s"
except TypeError as e:
    print("single-line:", type(e).__name__, e)
