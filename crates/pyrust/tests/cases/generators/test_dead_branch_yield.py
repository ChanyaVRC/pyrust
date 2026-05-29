# Regression test for issue #1758: a generator function whose `yield` appears
# only in compile-time-dead code (e.g. `if False: yield`) must still behave
# as a generator — calling it returns a generator object, and calling next()
# on that generator raises StopIteration.

# 1. Basic dead-branch yield.
def never_yields():
    if False:
        yield

g = never_yields()
try:
    next(g)
    print("FAIL: should raise StopIteration")
except StopIteration:
    print("ok")   # ok

# 2. Generator that is already done raises StopIteration again.
try:
    next(g)
    print("FAIL: exhausted generator should raise")
except StopIteration:
    print("ok-exhausted")  # ok-exhausted

# 3. elif False: yield.
def elif_dead_yield():
    if False:
        pass
    elif False:
        yield 99

g2 = elif_dead_yield()
try:
    next(g2)
    print("FAIL")
except StopIteration:
    print("ok-elif")  # ok-elif

# 4. list() on a generator that never yields produces [].
def always_empty():
    if False:
        yield 1

result = list(always_empty())
print(result)  # []

# 5. Normal generator (with reachable yield) must not regress.
def yields_one():
    yield 1

g3 = yields_one()
print(next(g3))  # 1
try:
    next(g3)
    print("FAIL: should exhaust")
except StopIteration:
    print("ok-normal")  # ok-normal

# 6. if True: yield (reachable) still works.
def yields_if_true():
    if True:
        yield 42

g4 = yields_if_true()
print(next(g4))  # 42
