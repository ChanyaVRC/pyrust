# Verify that exceptions raised inside generator bodies propagate correctly.
# The parity harness strips "Traceback ..." and "File ..." lines before
# diffing, so we compare the exception class and message — not the exact
# frame-chain header (which varies by path).  The underlying fix (issue #908)
# ensures the generator frame appears in the traceback, but the harness cannot
# diff it — manual inspection confirms CPython 3.12 parity.

# Basic case: exception raised in generator body caught by the caller.
def gen_raises():
    raise ValueError("inside generator")
    yield 1

try:
    for x in gen_raises():
        pass
except ValueError as e:
    print(type(e).__name__, str(e))

# Generator that yields once then raises.
def gen_yield_then_raise():
    yield 42
    raise RuntimeError("after yield")

g = gen_yield_then_raise()
try:
    first = next(g)
    print("first:", first)
    next(g)
except RuntimeError as e:
    print(type(e).__name__, str(e))

# Exception caught inside the generator should not propagate.
def gen_catches_internally():
    try:
        raise TypeError("internal")
    except TypeError:
        pass
    yield 1

results = list(gen_catches_internally())
print("internal catch:", results)

# StopIteration from normal generator exhaustion.
def gen_one():
    yield 99

g2 = gen_one()
print("value:", next(g2))
try:
    next(g2)
except StopIteration:
    print("StopIteration raised as expected")

# PEP 479: StopIteration escaping a generator body becomes RuntimeError.
def gen_stop_iter_escape():
    raise StopIteration("should be wrapped")
    yield

try:
    next(gen_stop_iter_escape())
except RuntimeError as e:
    print(type(e).__name__, "generator raised StopIteration" in str(e))

# Variadic generator function also raises correctly.
def gen_variadic(*args):
    raise KeyError(args[0])
    yield

try:
    next(gen_variadic("missing"))
except KeyError as e:
    print(type(e).__name__, e.args[0])

print("done")
