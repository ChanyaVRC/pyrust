# Parity fixture for generator.throw() — issue #787.
#
# Tests the standard single-argument form throw(exc_instance) which is the
# non-deprecated CPython 3.12 calling convention.  The two-arg form
# throw(ExcType, val) is deprecated in 3.12 and emits DeprecationWarning to
# stderr; it is exercised separately in the repro script but omitted here so
# the parity harness output is stable.

# One-arg form: exception instance carries the message.
def gen_instance():
    try:
        yield
    except ValueError as e:
        print(str(e))
        yield


g = gen_instance()
next(g)
try:
    g.throw(ValueError("oops"))
except StopIteration:
    pass

# One-arg form: exception class (no message).
def gen_class():
    try:
        yield
    except RuntimeError as e:
        print(repr(str(e)))  # ''
        yield


g2 = gen_class()
next(g2)
try:
    g2.throw(RuntimeError)
except StopIteration:
    pass

# One-arg form: exception propagates uncaught.
def gen_uncaught():
    yield


g3 = gen_uncaught()
next(g3)
try:
    g3.throw(TypeError("uncaught"))
except TypeError as e:
    print(str(e))

# One-arg form: generator re-raises a different exception.
def gen_reraise():
    try:
        yield
    except ValueError:
        raise RuntimeError("converted")


g4 = gen_reraise()
next(g4)
try:
    g4.throw(ValueError("trigger"))
except RuntimeError as e:
    print(str(e))

print("ok")
