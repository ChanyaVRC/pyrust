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
