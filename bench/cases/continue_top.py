# Issue #287: `if guard: continue` at loop top.  Should run within ~5% of the
# inverted form (continue_top_inverted.py) once the trampoline collapse lands.
i = 0
total = 0
while i < 10_000_000:
    if i % 2 == 0:
        i += 1
        continue
    total += i
    i += 1
