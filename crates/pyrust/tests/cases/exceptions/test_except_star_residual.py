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

# Case 6: residual PRESERVES the context that was active when the group was
# first raised.  The residual is the same object as the original group, so a
# pre-existing `__context__` (here the handled KeyError) survives the re-raise;
# only `__suppress_context__` is set.  Re-raising must NOT clobber it to None.
def residual_keeps_prior_context():
    try:
        raise KeyError("prior")
    except KeyError:
        try:
            raise ExceptionGroup("group", [ValueError(1), TypeError(2)])
        except* ValueError:
            pass


try:
    residual_keeps_prior_context()
except BaseException as e:
    print("ctx", repr(e.__context__))
    print("suppress", e.__suppress_context__)
    print("nsubs", len(e.exceptions))

# Case 7: when the residual is re-split by an outer `except*`, the matched
# subgroup is a *derived* group.  CPython's `exceptiongroup_subset` builds every
# derived subgroup with `__suppress_context__ is True` unconditionally, so the
# outer handler's bound group surfaces with `suppress True` (and `__context__`
# copied from the residual, here `None`).
def residual_suppress_after_outer_resplit():
    try:
        try:
            raise ExceptionGroup("g", [ValueError(1), TypeError(2)])
        except* ValueError:
            print("inner caught VE")
    except* TypeError as eg:
        print("outer subtypes", [type(x).__name__ for x in eg.exceptions])
        print("outer suppress", eg.__suppress_context__)
        print("outer ctx", repr(eg.__context__))


residual_suppress_after_outer_resplit()

# Case 8: a direct `.split()` / `.subgroup()` builds derived subgroups, each of
# which carries `__suppress_context__ is True` even when the source group's flag
# was the `False` default.  A whole-group match returns the source object
# unchanged, so it keeps the source's flag.
eg8 = ExceptionGroup("g", [ValueError(1), TypeError(2)])
m8, r8 = eg8.split(ValueError)
print("split match suppress", m8.__suppress_context__)
print("split rest suppress", r8.__suppress_context__)
print("subgroup suppress", eg8.subgroup(ValueError).__suppress_context__)
whole = eg8.split(Exception)[0]
print("whole-match is source", whole is eg8)
print("whole-match suppress", whole.__suppress_context__)

print("done")
