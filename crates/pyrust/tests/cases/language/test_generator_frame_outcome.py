# Parity tests for the FrameOutcome generator refactor (#451).
# These cover paths that were previously driven by GEN_SAVE thread-local
# and PyError::GeneratorYield; now driven by explicit FrameOutcome values.

# --- Basic yield / exhaust ---
def basic():
    yield 1
    yield 2

g = basic()
print(next(g))  # 1
print(next(g))  # 2
try:
    next(g)
except StopIteration:
    print("exhausted OK")

# --- Yield inside try/except carries exception context across resume ---
def with_exc_ctx():
    try:
        yield "a"
        raise ValueError("boom")
    except ValueError as e:
        yield "caught:" + str(e)

g2 = with_exc_ctx()
print(next(g2))   # a
print(next(g2))   # caught:boom
try:
    next(g2)
except StopIteration:
    print("exhausted OK")

# --- Yield inside finally: PEP 3134 context saved/restored correctly ---
def with_finally():
    try:
        yield 1
    finally:
        yield 2

g3 = with_finally()
print(next(g3))  # 1
print(next(g3))  # 2
try:
    next(g3)
except StopIteration:
    print("exhausted OK")

# --- yield from sub-generator: nested FrameOutcome threading ---
def inner():
    yield 10
    yield 20

def outer():
    yield from inner()
    yield 30

print(list(outer()))  # [10, 20, 30]

# --- Multiple independent generators do not share state ---
def counter(n):
    for i in range(n):
        yield i

g4 = counter(3)
g5 = counter(3)
print(next(g4))  # 0
print(next(g5))  # 0
print(next(g4))  # 1
print(next(g5))  # 1

# --- generator.throw() delivers exception; generator catches and yields again ---
def throw_target():
    try:
        yield 1
    except RuntimeError as e:
        yield "caught:" + str(e)

g6 = throw_target()
print(next(g6))                         # 1
print(g6.throw(RuntimeError("hi")))     # caught:hi

# --- generator.throw() on exhausted generator re-raises ---
def tiny():
    yield 1

g7 = tiny()
next(g7)
try:
    next(g7)
except StopIteration:
    pass
try:
    g7.throw(ValueError("late"))
except ValueError as e:
    print("late throw:", str(e))  # late throw: late

# --- generator.close() on suspended generator runs finally ---
log = []

def closeable():
    try:
        yield 1
        yield 2
    finally:
        log.append("finally")

g8 = closeable()
print(next(g8))    # 1
g8.close()
print(log)         # ['finally']

# --- collect_iterable path: list() drives generator via resume loop ---
def gen_abc():
    yield "x"
    yield "y"
    yield "z"

print(list(gen_abc()))   # ['x', 'y', 'z']
print(tuple(gen_abc()))  # ('x', 'y', 'z')

print("OK")
