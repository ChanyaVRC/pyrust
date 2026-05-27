# Regression test for the generator send() value corruption bug.
#
# When a function call with a computed argument appears between two yield
# expressions, pass_copy_prop previously propagated an alias from the
# yield-dst register to the arg-computation register through the Call
# instruction boundary.  On resume the Move reading from yield-dst was
# rewritten to read from the (stale) arg register instead, so the caller's
# sent value was silently discarded.

def gen():
    x = yield 1
    id(x + 0)   # computed arg — triggers the retarget/copy-prop path
    y = yield 2
    yield y

g = gen()
next(g)
g.send(10)
result = g.send(20)
print(result)   # 20

# Same pattern with x+1 (not an algebraic identity, exercises a different
# optimizer path that still triggered the same copy-prop aliasing bug).
def gen2():
    x = yield 1
    id(x + 1)
    y = yield 2
    yield y

g2 = gen2()
next(g2)
g2.send(10)
result2 = g2.send(20)
print(result2)  # 20

# f-string variant from the issue report.
def gen3():
    x = yield 1
    print(f"x={x}")
    y = yield 2
    yield y

g3 = gen3()
next(g3)
g3.send(10)
result3 = g3.send(20)
print(result3)  # 20

# Non-triggered case: literal arg — should not change register allocation.
def gen4():
    x = yield 1
    id("hello")
    y = yield 2
    yield y

g4 = gen4()
next(g4)
g4.send(10)
result4 = g4.send(20)
print(result4)  # 20

# Non-triggered case: named-variable arg — no retarget, no extra temp.
def gen5():
    x = yield 1
    msg = f"x={x}"
    print(msg)
    y = yield 2
    yield y

g5 = gen5()
next(g5)
g5.send(10)
result5 = g5.send(20)
print(result5)  # 20
