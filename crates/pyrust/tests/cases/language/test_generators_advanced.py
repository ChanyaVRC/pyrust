# Parity tests for generator edge cases identified in code review.

# --- x = yield value: sent value is None (send() not yet supported) ---
def echo_sent():
    x = yield 1
    yield x  # x should be None since only next() is used

g = echo_sent()
print(next(g))   # 1
print(next(g))   # None
try:
    next(g)
except StopIteration:
    print("StopIteration OK")

# --- yield from a generator (delegation to sub-generator) ---
def inner():
    yield 10
    yield 20

def outer():
    yield from inner()
    yield 30

result = list(outer())
print(result)  # [10, 20, 30]

# --- tuple() and set() consume a generator ---
def three_vals():
    yield "a"
    yield "b"
    yield "c"

print(tuple(three_vals()))        # ('a', 'b', 'c')
print(sorted(set(three_vals())))  # ['a', 'b', 'c']

# --- exception inside generator body ---
def gen_with_try():
    try:
        yield 1
        raise ValueError("oops")
    except ValueError:
        yield 2
    yield 3

g2 = gen_with_try()
print(next(g2))  # 1
print(next(g2))  # 2 (caught the ValueError, resumed)
print(next(g2))  # 3
try:
    next(g2)
except StopIteration:
    print("StopIteration OK")

# --- iter() on a user class calls __iter__() ---
class Counter:
    def __init__(self, n):
        self.i = 0
        self.n = n
    def __iter__(self):
        return self
    def __next__(self):
        if self.i >= self.n:
            raise StopIteration
        v = self.i
        self.i += 1
        return v

it = iter(Counter(3))
print(next(it))  # 0
print(next(it))  # 1
print(next(it))  # 2
try:
    next(it)
except StopIteration:
    print("StopIteration OK")

# --- iter() on a generator shares state (same underlying generator) ---
def simple():
    yield 42

g3 = simple()
it = iter(g3)
print(next(it))   # 42
try:
    next(it)
except StopIteration:
    print("StopIteration OK")

# --- next(iter(list)) works via NativeIterFrame ---
it2 = iter([7, 8, 9])
print(next(it2))          # 7
print(next(it2))          # 8
print(next(it2))          # 9
print(next(it2, "done"))  # done

# --- for loop over iter(list) ---
result2 = []
for x in iter([1, 2, 3]):
    result2.append(x)
print(result2)  # [1, 2, 3]

print("OK")
