# PEP 657 caret recovery when two or more nested sub-expressions both
# const-fold (issue #2586).
#
# PR #2579 kept the caret on a surviving binary op when ONE foldable sibling
# collapsed (suffix alignment).  When two (or more) siblings fold, the surviving
# binops are no longer a contiguous suffix of the original binops — an
# interspersed folded right-subtree opens a gap — so the suffix recovery bails
# and the op previously lost its caret.  The left-spine recovery now aligns each
# survivor to the expression's left-edge op chain.
#
# NOTE: the parity harness strips the `^`/`~` underline rows before diffing, so
# this fixture pins the **source-line / line-number / message** parity.  The
# precise caret columns are asserted byte-for-byte against python3.12 in the
# optimizer unit tests (`multifold_caret_recovers_left_spine`) and were verified
# manually here.

# --- both inner sub-expressions fold; the left `+` raises (str + int) ---
try:
    x = (2 + 3) + "s" + (5 + 7)
except TypeError as e:
    print("both-fold:", type(e).__name__, e)


# --- left sub-expression folds twice down the spine; outer `+` raises ---
try:
    y = (2 + 3) * 4 + "s"
except TypeError as e:
    print("spine-fold:", type(e).__name__, e)


# --- three folded siblings, trailing operand raises ---
try:
    z = (1 + 2) + (3 + 4) + (5 + 6) + "s"
except TypeError as e:
    print("triple-fold:", type(e).__name__, e)


# --- interspersed string operands between folds ---
try:
    w = "a" + (2 + 3) + "b" + (4 + 5) + "c"
except TypeError as e:
    print("interspersed:", type(e).__name__, e)


# --- right-operand subtree survives (no fold there): must stay correct ---
a = 10
b = 20
try:
    q = (2 + 3) + (a + b) + "s"
except TypeError as e:
    print("right-survivor:", type(e).__name__, e)
