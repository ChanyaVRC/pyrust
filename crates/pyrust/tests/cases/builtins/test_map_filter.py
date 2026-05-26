# Parity fixture: map() and filter() lazy iterators.
# Covers: multiple iterables, lazy semantics, type names, exhaustion,
# next() protocol, for-loop iteration, and generator sources.

# --- map: single iterable (original behaviour) ---
print(list(map(str, [1, 2, 3])))

# --- map: two iterables ---
print(list(map(lambda x, y: x + y, [1, 2, 3], [10, 20, 30])))

# --- map: three iterables ---
print(list(map(lambda x, y, z: x + y + z, [1, 2], [10, 20], [100, 200])))

# --- map: stops at shortest iterable ---
print(list(map(lambda x, y: (x, y), [1, 2, 3], [10, 20])))

# --- map: empty iterable ---
print(list(map(lambda x: x, [])))

# --- map: empty shortest ---
print(list(map(lambda x, y: x + y, [1, 2, 3], [])))

# --- map: lazy (type name is 'map', not 'list') ---
m = map(str, [1, 2, 3])
print(type(m).__name__)

# --- map: next() protocol ---
m = map(str, [1, 2, 3])
print(next(m))
print(next(m))
print(list(m))

# --- map: next() with default on exhaustion ---
m = map(str, [1])
print(next(m))
print(next(m, 'done'))

# --- map: StopIteration after exhaustion ---
m = map(str, [1])
next(m)
try:
    next(m)
except StopIteration:
    print('StopIteration')

# --- map: for-loop iteration ---
for v in map(lambda x: x * 2, [1, 2, 3]):
    print(v)

# --- map: generator source ---
def gen():
    yield 10
    yield 20

print(list(map(lambda x: x + 1, gen())))

# --- map: error — too few arguments ---
try:
    map(str)
except TypeError as e:
    print(type(e).__name__, e)

# --- filter: identity (func=None) ---
f = filter(None, [0, 1, 2, 0, 3])
print(type(f).__name__)
print(list(f))

# --- filter: with function ---
print(list(filter(lambda x: x > 2, [1, 2, 3, 4, 5])))

# --- filter: empty iterable ---
print(list(filter(None, [])))

# --- filter: all falsy ---
print(list(filter(None, [0, False, '', None])))

# --- filter: next() protocol ---
f = filter(None, [0, 1, 2])
print(next(f))
print(next(f))

# --- filter: StopIteration after exhaustion ---
f = filter(None, [1])
next(f)
try:
    next(f)
except StopIteration:
    print('StopIteration')

# --- filter: next() with default ---
f = filter(None, [1])
print(next(f))
print(next(f, 'done'))

# --- filter: generator source ---
def gen2():
    yield 0
    yield 1
    yield 2

print(list(filter(None, gen2())))

# --- map: func raises mid-iteration; iterator advances past the bad element ---
def raise_on_two(x):
    if x == 2:
        raise ValueError('bad')
    return x

m = map(raise_on_two, [1, 2, 3])
print(next(m))
try:
    next(m)
except ValueError:
    print('ValueError')
print(next(m))

# --- filter: func raises mid-iteration; iterator advances past the bad element ---
f = filter(raise_on_two, [1, 2, 3])
print(next(f))
try:
    next(f)
except ValueError:
    print('ValueError')
print(next(f))
