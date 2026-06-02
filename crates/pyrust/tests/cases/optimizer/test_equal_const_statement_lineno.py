# Regression test for issue #1962.
#
# The optimizer folds `2**1024` and `(2**512)*(2**512)` to the same constant
# value and deduplicates the constant-pool slot.  A bug in the post-optimization
# line-number remap (it ran after constant-pool compaction) let a reindexed
# constant spuriously match an unrelated original instruction, dragging the
# greedy match cursor forward.  As a result the *first* statement's overflowing
# division was attributed to the *second* statement, so the earlier statement
# appeared to be skipped and the exception reported the wrong line.
#
# Each top-level statement is its own evaluation point: an exception in the first
# equal-valued constant expression must fire there, and a statement that runs
# before it must have executed.  We verify this through observable side effects
# (a log list) rather than traceback introspection, which keeps the fixture
# stable across interpreters.


# Two equal-valued huge constants in separate statements; the first must raise,
# so the marker appended just before it runs but the second statement's marker
# must NOT, because evaluation stops at the first raise.
def equal_constants_first_raises():
    log = []
    try:
        log.append("before-1")
        repr((2**1024) / 1)          # raises OverflowError here
        log.append("between")
        repr(((2**512) * (2**512)) / 1)
        log.append("after-2")
    except OverflowError:
        log.append("caught")
    return log


print("equal-valued:", equal_constants_first_raises())


# Control: distinct magnitudes already behaved correctly; verify still does.
def distinct_constants_first_raises():
    log = []
    try:
        log.append("before-1")
        repr((10**400) / 1)          # raises OverflowError here
        log.append("between")
        repr((10**500) / 1)
        log.append("after-2")
    except OverflowError:
        log.append("caught")
    return log


print("distinct:", distinct_constants_first_raises())


# A trailing statement after the two equal constants must not change behaviour.
def with_trailing():
    log = []
    try:
        log.append("before-1")
        repr((2**1024) / 1)          # raises OverflowError here
        log.append("between")
        repr(((2**512) * (2**512)) / 1)
        repr("unreached")
    except OverflowError:
        log.append("caught")
    return log


print("with trailing:", with_trailing())


# Equal-valued non-raising constants in separate statements: both evaluate, the
# legitimate constant-fold optimization still applies, and the values agree.
def equal_constants_no_raise():
    a = (2**1024) // 1
    b = ((2**512) * (2**512)) // 1
    return a == b and a == 2**1024


print("equal non-raising consts agree:", equal_constants_no_raise())


# Side-effect ordering: two statements that each have an observable effect must
# each run, in order, even when they compute equal values.
def both_run():
    log = []
    log.append((2**64).bit_length())              # 65
    log.append(((2**32) * (2**32)).bit_length())  # 65
    return log


print("both side-effects ran:", both_run())


# Sanity: ordinary small-constant folding still works at runtime.
print("small const fold:", 2 + 3, "a" + "b")
