# Parity test: yield / yield from (generators)

# --- Simple generator ---
def count_up(n):
    for i in range(n):
        yield i

g = count_up(3)
print(next(g))  # 0
print(next(g))  # 1
print(next(g))  # 2
try:
    next(g)
    print("ERROR: should have raised StopIteration")
except StopIteration:
    print("StopIteration OK")

# --- Generator in for loop ---
def squares(n):
    for i in range(n):
        yield i * i

result = []
for x in squares(4):
    result.append(x)
print(result)  # [0, 1, 4, 9]

# --- next() with default ---
g2 = count_up(1)
print(next(g2))           # 0
print(next(g2, "done"))   # done (exhausted, returns default)

# --- bare yield (yields None) ---
def gen_none():
    yield
    yield

g3 = gen_none()
print(next(g3))  # None
print(next(g3))  # None
try:
    next(g3)
    print("ERROR")
except StopIteration:
    print("StopIteration OK")

# --- yield from a list ---
def yield_from_list():
    yield from [10, 20, 30]

result2 = list(yield_from_list())
print(result2)  # [10, 20, 30]

# --- yield from a range ---
def yield_from_range():
    yield from range(3)

result3 = list(yield_from_range())
print(result3)  # [0, 1, 2]

# --- generator with return (StopIteration) ---
def gen_return():
    yield 1
    yield 2
    return  # explicit return stops generator

g4 = gen_return()
print(next(g4))  # 1
print(next(g4))  # 2
try:
    next(g4)
    print("ERROR")
except StopIteration:
    print("StopIteration OK")

# --- list() consumes a generator ---
def counter(n):
    i = 0
    while i < n:
        yield i
        i += 1

print(list(counter(5)))  # [0, 1, 2, 3, 4]

# --- multiple generators are independent ---
g5 = count_up(3)
g6 = count_up(3)
print(next(g5))  # 0
print(next(g6))  # 0
print(next(g5))  # 1
print(next(g6))  # 1

print("OK")
