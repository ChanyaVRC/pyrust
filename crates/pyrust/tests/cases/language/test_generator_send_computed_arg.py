# Regression test for the bug where a computed arg to a function call between
# two yield expressions causes copy-propagation to alias the yield-dst register
# with the temp register used to hold the computed arg, so that the sent value
# from the second .send() call is silently replaced by the computed arg value.

# ── basic repro from issue #1438 ─────────────────────────────────────────────
def gen_basic():
    x = yield 1
    id(x + 0)   # computed arg: triggers the copy-prop alias bug
    y = yield 2
    yield y

g = gen_basic()
next(g)
g.send(10)        # advances to 'yield 2'
result = g.send(20)
assert result == 20, f"expected 20 got {result}"

# ── variant: multiple computed args ──────────────────────────────────────────
def gen_multi_arg():
    x = yield 1
    id(x + 0)
    id(x + 1)
    y = yield 2
    yield y

g = gen_multi_arg()
next(g)
g.send(10)
result = g.send(30)
assert result == 30, f"expected 30 got {result}"

# ── variant: computed arg is a list literal containing a local ───────────────
def gen_list_arg():
    x = yield 1
    id([x])
    y = yield 2
    yield y

g = gen_list_arg()
next(g)
g.send(10)
result = g.send(40)
assert result == 40, f"expected 40 got {result}"

# ── variant: computed arg is an f-string ─────────────────────────────────────
def gen_fstring_arg():
    x = yield 1
    len(f"x={x}")
    y = yield 2
    yield y

g = gen_fstring_arg()
next(g)
g.send(10)
result = g.send(50)
assert result == 50, f"expected 50 got {result}"

# ── non-trigger: literal arg (no aliasing, copy-prop should be fine) ─────────
def gen_literal_arg():
    x = yield 1
    id("hello")   # literal arg — no temp register involved
    y = yield 2
    yield y

g = gen_literal_arg()
next(g)
g.send(10)
result = g.send(60)
assert result == 60, f"expected 60 got {result}"

# ── non-trigger: named variable (no retarget, no alias) ──────────────────────
def gen_named_var():
    x = yield 1
    msg = f"x={x}"
    len(msg)
    y = yield 2
    yield y

g = gen_named_var()
next(g)
g.send(10)
result = g.send(70)
assert result == 70, f"expected 70 got {result}"

# ── three yields with computed arg between each pair ─────────────────────────
def gen_three():
    x = yield 1
    id(x + 0)
    y = yield 2
    id(y + 0)
    z = yield 3
    yield z

g = gen_three()
next(g)
g.send(10)
g.send(20)
result = g.send(30)
assert result == 30, f"expected 30 got {result}"

print("generator send computed arg OK")
