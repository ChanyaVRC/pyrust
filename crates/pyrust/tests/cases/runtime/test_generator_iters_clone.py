# Parity test: generator yield correctness when iters and exc_handlers
# stacks are non-empty at the yield point (regression for the mem::take
# optimisation in issue #515).

# --- Basic generator ---
def gen_basic():
    yield 1
    yield 2

print(list(gen_basic()))

# --- For-loop inside generator (iters stack non-empty on yield) ---
def gen_for():
    for x in range(10):
        yield x

print(list(gen_for()))

# --- try/except inside generator (exc_handlers stack non-empty on yield) ---
def gen_try():
    try:
        yield 1
    except Exception:
        pass

print(list(gen_try()))

# --- Combined: for-loop + try/except (both stacks non-empty on yield) ---
def gen_for_try():
    for i in range(5):
        try:
            yield i
        except Exception:
            pass

print(list(gen_for_try()))

# --- Generator with close() after partial iteration (iters stack must be
#     correctly restored on resume so finally blocks can run) ---
def gen_close():
    try:
        for i in range(100):
            yield i
    finally:
        print("gen_close finally")

g = gen_close()
print(next(g))
print(next(g))
g.close()

# --- Generator with throw() while inside for-loop ---
def gen_throw():
    for i in range(5):
        try:
            yield i
        except ValueError as e:
            print("caught", e)
            yield -1

g = gen_throw()
print(next(g))          # yields 0 (i=0)
print(g.throw(ValueError("boom")))  # caught, yields -1
print(next(g))          # yields 1 (i=1, for-loop continues)

# --- Nested generators: yield from (outer has non-empty iters on yield) ---
def inner():
    yield "a"
    yield "b"

def outer():
    for x in inner():
        yield x
    yield "c"

print(list(outer()))
