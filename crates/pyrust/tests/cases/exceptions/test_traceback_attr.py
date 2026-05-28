# Issue #1052: e.__traceback__ raises AttributeError on any exception instance.
# CPython 3.12 always initialises __traceback__ to None; with_traceback() must
# set it and return self.

# Freshly constructed exception: __traceback__ is None (not AttributeError).
e = ValueError("test")
print(e.__traceback__ is None)   # True
print(e.__traceback__)           # None

# Caught exception: __traceback__ is a traceback object (not None, not AttributeError).
try:
    raise ValueError("test")
except ValueError as e:
    print(type(e.__traceback__).__name__)   # traceback

# with_traceback(None) pattern used in re-raise idioms must not crash.
try:
    raise ValueError("original")
except ValueError as e:
    try:
        raise RuntimeError("wrapped") from e.with_traceback(None)
    except RuntimeError as re:
        print(type(re).__name__)            # RuntimeError
        print(re.__context__ is not None)   # True

# All exception subclasses initialise __traceback__ to None.
for exc_class in (KeyError, TypeError, AttributeError, IndexError, OSError):
    inst = exc_class("msg")
    print(inst.__traceback__ is None)      # True

# with_traceback() returns self (same identity).
e = ValueError("x")
e2 = e.with_traceback(None)
print(e is e2)                             # True
print(e2.__traceback__ is None)            # True
