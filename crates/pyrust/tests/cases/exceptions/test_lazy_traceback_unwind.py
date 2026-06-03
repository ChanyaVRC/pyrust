# Exercises the lazy traceback machinery: traceback frames are now recorded
# only as an exception unwinds through nested user-function calls, instead of
# being pushed/popped eagerly on every call (perf: reduce per-call overhead).
#
# pyrust's stderr traceback format is not byte-for-byte CPython yet (inner
# frames omit line numbers), so this fixture catches every exception and only
# asserts the stdout-observable control flow: that the right exception reaches
# the right handler, that a caught inner exception does not leak into a later
# unrelated error, and that bare re-raise propagates the original.


# --- exception unwinds through several nested calls and is caught ---
def a():
    raise ValueError("boom")


def b():
    a()


def c():
    b()


try:
    c()
except ValueError as e:
    print("caught:", e)  # caught: boom


# --- a caught-and-handled inner exception must NOT mask the later error ---
def inner_caught():
    raise KeyError("swallowed")


def raises_later():
    try:
        inner_caught()
    except KeyError:
        pass
    raise RuntimeError("the real one")


def wrap():
    raises_later()


try:
    wrap()
except RuntimeError as e:
    print("late:", e)  # late: the real one
except KeyError as e:
    print("WRONG:", e)


# --- bare re-raise inside a handler propagates the original exception ---
def deep():
    raise IndexError("deep")


def mid():
    try:
        deep()
    except IndexError:
        raise


try:
    mid()
except IndexError as e:
    print("reraised:", e)  # reraised: deep


# --- many successful no-error calls, then a single late error ---
def ok(x):
    return x + 1


total = 0
for i in range(50):
    total += ok(i)
print(total)  # 1275

try:

    def boom():
        raise TypeError("late typeerror")

    boom()
except TypeError as e:
    print("final:", e)  # final: late typeerror
