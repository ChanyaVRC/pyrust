# Parity test for issue #488: variadic-arg generator functions must
# return an iterator object instead of running the body synchronously
# at call time.

# --- *args generator ---
def g1(*args):
    for a in args:
        yield a

print(list(g1(1, 2, 3)))  # [1, 2, 3]

# Iter-protocol via for-loop.
out = []
for x in g1(11, 12):
    out.append(x)
print(out)  # [11, 12]

# Single-step via __next__.
it = g1(7, 8, 9)
print(it.__next__(), it.__next__(), it.__next__())  # 7 8 9
try:
    it.__next__()
    print("ERROR: should have raised StopIteration")
except StopIteration:
    print("StopIteration OK")

# --- **kwargs generator ---
def g2(**kwargs):
    for k in kwargs:
        yield (k, kwargs[k])

print(list(g2(a=1, b=2)))  # [('a', 1), ('b', 2)]

# --- mixed (a, *args) ---
def g3(a, *args):
    yield a
    for x in args:
        yield x

print(list(g3(10, 20, 30)))  # [10, 20, 30]

# --- a, *args, **kwargs ---
def g4(a, *args, **kwargs):
    yield a
    for x in args:
        yield x
    for k in kwargs:
        yield (k, kwargs[k])

print(list(g4(1, 2, 3, x=10, y=20)))  # [1, 2, 3, ('x', 10), ('y', 20)]

# --- baseline: no args, still works ---
def g5():
    yield 1
    yield 2

print(list(g5()))  # [1, 2]

# --- locals() inside variadic generator: must surface the
#     generator's own fastlocals (args tuple, x), not the caller's. ---
def g6(*args):
    x = 42
    yield locals()

loc = next(g6(100, 200))
print(sorted(loc.keys()))  # ['args', 'x']
print(loc["args"])         # (100, 200)
print(loc["x"])            # 42

# --- nonlocal cell-var capture: a variadic generator that closes
#     over an outer cell. Verifies the env-swap dance in the variadic
#     branch keeps the enclosing closure scope reachable through the
#     GeneratorFrame's `saved_env`. ---
def outer():
    x = 0
    def gen(*args):
        nonlocal x
        for a in args:
            x += a
            yield x
    return gen

g = outer()
print(list(g(1, 2, 3)))  # [1, 3, 6]

# --- throw() into a variadic generator: exception propagates through
#     the generator body's try/except, exercising the resume path. ---
def g7(*args):
    try:
        yield args
    except ValueError as e:
        yield ("caught", str(e))

it = g7(11, 22)
print(next(it))                       # (11, 22)
print(it.throw(ValueError("boom")))   # ('caught', 'boom')
