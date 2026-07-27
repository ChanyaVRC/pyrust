x = 1


def read():
    return x


def write():
    global x
    x = 2


# Prime read.__code__ while this filesystem module is still executing through
# the import child Interpreter.  The caller later reaches the same function and
# root namespace through its parent Interpreter.
primed = read()

# Expose this module's own globals backing before the child Interpreter goes
# away.  The returned alias must remain authoritative when read() is later
# invoked by the importing parent.
g = globals()
