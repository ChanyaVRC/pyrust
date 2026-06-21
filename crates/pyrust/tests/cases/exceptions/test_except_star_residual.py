# Parity fixture: PEP 654 `except*` residual (leftover) sub-exception group.
#
# When an `except*` clause handles some but not all sub-exceptions of an
# ExceptionGroup, the unhandled sub-exceptions are re-raised as a residual
# group at the end of the except* block.  CPython re-raises that residual
# *without* implicit-context chaining: the residual surfaces with
# `__context__ is None` and `__suppress_context__ is True`, and it carries only
# the UNHANDLED sub-exceptions (so its sub-exception count is the leftover
# count, not the original count).  See issue #2755.

# Case 1: partial match — residual carries only the unhandled sub-exception,
# with no spurious implicit context.
def partial_match():
    try:
        raise ExceptionGroup("group", [ValueError(1), TypeError(2)])
    except* ValueError as eg:
        print("caught", eg.exceptions)


try:
    partial_match()
except BaseException as e:
    print("type", type(e).__name__)
    print("nsubs", len(e.exceptions))
    print("subtypes", [type(x).__name__ for x in e.exceptions])
    print("context", e.__context__)
    print("cause", e.__cause__)
    print("suppress", e.__suppress_context__)

# Case 2: the residual's __traceback__ chain is the original group's chain,
# with no duplicated re-raise frame.
def f():
    raise ExceptionGroup("group", [ValueError(1), TypeError(2)])


def g():
    try:
        f()
    except* ValueError:
        pass


try:
    g()
except BaseException as e:
    frames = []
    tb = e.__traceback__
    while tb:
        frames.append((tb.tb_frame.f_code.co_name, tb.tb_lineno))
        tb = tb.tb_next
    print("frames", frames)

# Case 3: two handlers, leaving a residual of a third class.
def two_handlers():
    try:
        raise ExceptionGroup("g", [ValueError(1), TypeError(2), KeyError(3)])
    except* ValueError as eg:
        print("VE", [type(x).__name__ for x in eg.exceptions])
    except* TypeError as eg:
        print("TE", [type(x).__name__ for x in eg.exceptions])


try:
    two_handlers()
except BaseException as e:
    print("residual", [type(x).__name__ for x in e.exceptions])
    print("residual nsubs", len(e.exceptions))
    print("residual suppress", e.__suppress_context__)

# Case 4: all sub-exceptions matched — no residual, clean exit.
try:
    raise ExceptionGroup("g", [ValueError(1), ValueError(2)])
except* ValueError as eg:
    print("all caught", len(eg.exceptions))
print("clean exit after full match")

# Case 5: residual re-raised and caught by an outer except* handler.
try:
    try:
        raise ExceptionGroup("g", [ValueError(1), TypeError(2)])
    except* ValueError:
        print("inner caught VE")
except* TypeError as eg:
    print("outer caught", [type(x).__name__ for x in eg.exceptions])

print("done")
