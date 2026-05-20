# Parity fixture for the LoadNoneRange opcode / pass_loadnone_merge optimizer pass.
#
# The pass fuses consecutive LoadNone(r), LoadNone(r+1), ... LoadNone(r+N-1)
# into a single LoadNoneRange { start: r, count: N } instruction.
# This is a pure optimization — observable behavior must be identical to
# running each LoadNone individually.


def many_locals(condition):
    # All locals initialized to None; the optimizer will fuse the prologue.
    a = None
    b = None
    c = None
    d = None
    e = None
    f = None
    if condition:
        a = 1
        b = 2
        c = 3
        d = 4
        e = 5
        f = 6
    return a, b, c, d, e, f


print(many_locals(True))   # (1, 2, 3, 4, 5, 6)
print(many_locals(False))  # (None, None, None, None, None, None)


# Verify None is the correct default when the branch is not taken.
def check_none():
    x = None
    if x is None:
        return "yes"
    return "no"


print(check_none())  # yes


# A function whose locals are all permanently None.
def all_none():
    a = None
    b = None
    c = None
    return a, b, c


print(all_none())  # (None, None, None)


# Non-consecutive register assignments must not be merged.
# Here only some locals are None; others have values.
def mixed_init():
    a = 1
    b = None
    c = 2
    d = None
    return a, b, c, d


print(mixed_init())  # (1, None, 2, None)
