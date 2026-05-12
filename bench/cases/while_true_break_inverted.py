# Issue #282 reference: the manually inverted equivalent of
# while_true_break_top.py.  `while i < 10_000_000: body` is the canonical
# shape that `try_compile_while_range` promotes to `ForCountConstInline`;
# the AST rewrite for `while True: if c: break; body` aims to match this
# form's bytecode (and its runtime).
i = 0
total = 0
while i < 10_000_000:
    total += i
    i += 1
