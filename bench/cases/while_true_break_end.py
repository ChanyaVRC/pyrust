# Issue #282: `while True: body; if c: break` with the break at the end.
# This shape is not handled by the current AST rewrite (the loop-rotate
# variant), but is included for reference / future work — it sits between
# the trampoline form and the canonical `while not c: body` shape.
i = 0
total = 0
while True:
    total += i
    i += 1
    if i >= 10_000_000:
        break
