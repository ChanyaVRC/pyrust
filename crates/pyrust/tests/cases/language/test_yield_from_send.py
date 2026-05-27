# PEP 380 yield from: per-call send() value propagation.
# Regression test for issue #1439: pass_copy_prop treated the Yield dst
# register as a static copy source, causing stale aliases after a subsequent
# Yield overwrote that register with a new sent value.

# --- 1. Two-level: distinct sent values reach each yield in sub ---

def sub():
    x = yield 1
    y = yield 2
    return (x, y)

def wrapper():
    result = yield from sub()
    yield result

g = wrapper()
next(g)        # prime: sub yields 1
g.send(10)     # delivers 10 as x; sub yields 2
result = g.send(20)  # delivers 20 as y; sub returns (10, 20)
print(result)  # (10, 20)


# --- 2. Three-level delegation ---

def sub2():
    a = yield 'a'
    b = yield 'b'
    return [a, b]

def mid():
    r = yield from sub2()
    yield r

def outer():
    r = yield from mid()
    yield r

g2 = outer()
next(g2)
g2.send('x')
result2 = g2.send('y')
print(result2)  # ['x', 'y']


# --- 3. yield from with only next() (no send) still works ---

def sub3():
    yield 10
    yield 20
    return 'done'

def wrapper3():
    r = yield from sub3()
    yield r

print(list(wrapper3()))  # [10, 20, 'done']


# --- 4. StopIteration return value propagation ---

def sub4():
    yield 1
    return 42

def wrapper4():
    v = yield from sub4()
    yield v

g4 = wrapper4()
print(next(g4))   # 1
print(next(g4))   # 42
