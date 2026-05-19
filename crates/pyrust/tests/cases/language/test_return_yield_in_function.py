# Parity fixture for issue #788: return and yield are valid inside functions
# and generators.  The error case (return/yield at module level raising
# SyntaxError) is tested via unit tests in interpreter/tests.rs because the
# parity harness cannot diff scripts that exit non-zero.

# return inside a function.
def f(x):
    if x > 0:
        return x
    return 0

print(f(5))   # 5
print(f(-1))  # 0

# return inside a nested loop inside a function.
def find(lst, x):
    for i, v in enumerate(lst):
        if v == x:
            return i
    return -1

print(find([10, 20, 30], 20))   # 1
print(find([10, 20, 30], 99))   # -1

# yield inside a generator function.
def gen():
    yield 1
    yield 2
    yield 3

print(list(gen()))  # [1, 2, 3]

# yield from inside a generator.
def gen2():
    yield from range(3)

print(list(gen2()))  # [0, 1, 2]

# return inside a nested function at module level: the outer is a function,
# so return is valid.
def outer():
    def inner():
        return 42
    return inner()

print(outer())  # 42

# yield in a generator defined inside a class method.
class Counter:
    def items(self):
        yield 'a'
        yield 'b'

c = Counter()
print(list(c.items()))  # ['a', 'b']
