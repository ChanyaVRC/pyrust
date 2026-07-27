# Issue #282 reference: the manually inverted equivalent of
# while_true_break_top.py.  `while i < 10_000_000: body` is the canonical
# source shape whose bytecode and runtime the top-break simplification aims
# to match.
i = 0
total = 0
while i < 10_000_000:
    total += i
    i += 1
