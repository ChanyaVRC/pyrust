# Issue #287 reference: the manually inverted equivalent of continue_top.py.
# `if i % 2 != 0: body` runs without the JumpIfFalse + Jump trampoline that the
# `if i % 2 == 0: continue` shape used to emit.
i = 0
total = 0
while i < 10_000_000:
    if i % 2 != 0:
        total += i
    i += 1
