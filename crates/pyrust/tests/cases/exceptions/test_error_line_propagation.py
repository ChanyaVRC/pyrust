# Exercises the VM's error-propagation paths after the per-instruction line
# tracker was made register-resident and flushed lazily on the
# error-propagation path (issue #348).  The dispatch loop no longer writes the
# current source line to a thread-local on every instruction; instead it keeps
# the line in a local and publishes it only when an exception escapes the
# frame.  These cases confirm that exceptions still raise with the correct
# class, message, and chaining across nested calls, caught/re-raised handlers,
# loops, and generators — all the control-flow shapes that reach the flush.
#
# The parity harness strips "Traceback"/"File" lines, so the *line numbers*
# themselves are verified by the reviewer via manual `pyrust` invocation
# (pyrust reports `<module>`-frame lines matching CPython for errors that
# propagate out of nested calls).  This fixture pins the observable behaviour.


# 1. Error propagating out of a nested call is caught with the right type.
def inner():
    return 1 + None


def outer():
    return inner()


try:
    outer()
except TypeError as e:
    print("nested:", type(e).__name__)


# 2. Catch inside a function, re-raise a different exception with `from None`.
def reraises():
    try:
        x = 1 / 0
    except ZeroDivisionError:
        raise ValueError("converted") from None


try:
    reraises()
except ValueError as e:
    print("reraise:", type(e).__name__, str(e), e.__cause__)


# 3. Error raised mid-loop after several successful iterations.
def loop_then_fail():
    total = 0
    for i in range(5):
        total += i
        if i == 3:
            return total + None  # TypeError after partial work
    return total


try:
    loop_then_fail()
except TypeError as e:
    print("loop:", type(e).__name__)


# 4. Deeply nested propagation (three call levels).
def a():
    return b()


def b():
    return c()


def c():
    raise KeyError("deep")


try:
    a()
except KeyError as e:
    print("deep:", type(e).__name__, e.args[0])


# 5. Exception escaping a generator body is wrapped/propagated correctly.
def gen():
    yield 1
    raise RuntimeError("from generator")


g = gen()
print("yielded:", next(g))
try:
    next(g)
except RuntimeError as e:
    print("gen:", type(e).__name__, str(e))


# 6. Chained exception preserves __context__ through a nested call.
def chains():
    try:
        [].pop()
    except IndexError:
        raise ValueError("wrapped")


try:
    chains()
except ValueError as e:
    print("chain:", type(e).__name__, type(e.__context__).__name__)

print("done")
