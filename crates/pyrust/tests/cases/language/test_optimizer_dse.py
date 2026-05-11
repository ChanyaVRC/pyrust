# Dead store elimination: registers whose written value is overwritten before
# being read should be silently removed by the optimizer.

# ── overwritten before read ────────────────────────────────────────────────
def overwrite():
    x = 1    # dead store — x is overwritten below before any use
    x = 2
    return x

assert overwrite() == 2

# ── assignment in conditional branch ──────────────────────────────────────
def conditional(flag):
    y = 0    # may be dead if the branch executes before y is read
    if flag:
        y = 10
    return y

assert conditional(True) == 10
assert conditional(False) == 0

# ── chained overwrites ────────────────────────────────────────────────────
def chain():
    a = 1
    a = 2
    a = 3
    return a

assert chain() == 3

# ── arithmetic result overwritten ─────────────────────────────────────────
def arith():
    t = 1 + 1   # dead — t overwritten immediately
    t = 42
    return t

assert arith() == 42

# ── dict literal (BuildDict reads are not incorrectly eliminated) ─────────
d = {'x': 1, 'y': 2}
assert d['x'] == 1
assert d['y'] == 2

# ── **kwargs expansion still works (DictUpdate receiver safety) ────────────
def merge(**kw):
    return kw

r = merge(**{'a': 1}, **{'b': 2})
assert r == {'a': 1, 'b': 2}, f"got {r}"

# ── loop variable not eliminated ──────────────────────────────────────────
total = 0
for i in range(5):
    total += i
assert total == 10

print("dse ok")
