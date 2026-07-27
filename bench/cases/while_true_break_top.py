# Issue #282: `while True: if c: break; body` with the break at the top of
# the body.  Should run within ~5% of the equivalent `while not c: body`
# form after control-flow simplification.
i = 0
total = 0
while True:
    if i >= 10_000_000:
        break
    total += i
    i += 1
