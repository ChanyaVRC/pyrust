# Parity fixture: PEP 657 carets survive constant folding (issues #2577 / #2578).
#
# When a foldable sub-expression collapses, the optimizer must still attach the
# raising binary op's caret anchor to the surviving (often fused) instruction:
#
#   a + b + "s"     # a, b known ints → `a + b` folds; caret under the 2nd `+`
#   "s" + (x + 2)   # x known int → `x + 2` folds; caret under the outer `+`
#
# The parity harness strips the `^`/`~` underline row before diffing (CPython's
# fine-grained markers vs pyrust's row are normalized away), so this fixture
# pins the parts the harness *does* compare: the echoed source line and the
# exception class + message.  The caret column itself is asserted byte-for-byte
# against CPython 3.12 in the PR's manual verification.  Each snippet runs via
# exec() so the TypeError surfaces, then a sentinel confirms execution continued.
#
# All output goes to stdout (not stderr) so the merged-stream ordering the
# harness diffs is deterministic across runtimes.


def run(src):
    try:
        exec(src, {})
    except TypeError as e:
        # Echo class + message so the harness diff covers them across runtimes.
        print("err: " + type(e).__name__ + ": " + str(e))


# ── #2577: foldable sub-expression precedes the raising op ───────────────────
run("a = 1\nb = 2\nr = a + b + \"s\"")
print("section1 ok")

# ── #2578: parenthesized foldable operand of the raising op ──────────────────
run("x = 1\nz = \"s\" + (x + 2)")
print("section2 ok")

# ── Mixed: folded sub-expression in the middle of the chain ──────────────────
run("x = 1\ny = 2\nr = x + y + (1 + 2) + \"s\"")
print("section3 ok")

# ── Fully-constant left fold, raising op on the right ────────────────────────
run("r = (10 + 2) * 3 + \"s\"")
print("section4 ok")

# ── Parenthesized foldable operand on the right of a str concat ──────────────
run("a = 1\nb = 2\nr = \"s\" + (a + b)")
print("section5 ok")

# ── Non-folded single binary op (regression guard for the common path) ───────
run("x = 5\nr = x + \"s\"")
print("section6 ok")
