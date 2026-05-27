# Issue #1441: Exception instances must expose __traceback__ and with_traceback().

# Fresh exception has __traceback__ == None (not AttributeError).
e = RuntimeError("test")
print(e.__traceback__ is None)  # True

# hasattr works without raising AttributeError.
print(hasattr(e, "__traceback__"))  # True

# Caught exception has a non-None __traceback__.
try:
    1 / 0
except ZeroDivisionError as exc:
    tb = exc.__traceback__
    print(tb is not None)        # True
    print(type(tb).__name__)     # traceback

# with_traceback(None) returns self and sets __traceback__ = None.
e2 = ValueError("x").with_traceback(None)
print(type(e2).__name__)         # ValueError
print(e2.__traceback__ is None)  # True

# raise val.with_traceback(None) works end-to-end.
try:
    raise ValueError("chain").with_traceback(None)
except ValueError as exc:
    print(type(exc).__name__)           # ValueError
    print(exc.__traceback__ is not None)  # True (set when caught)

# with_traceback() rejects non-traceback, non-None values.
try:
    ValueError("x").with_traceback(42)
except TypeError as te:
    print(str(te))  # __traceback__ must be a traceback or None

# Assigning a non-traceback, non-None to __traceback__ raises TypeError.
try:
    e3 = TypeError("t")
    e3.__traceback__ = "bad"
except TypeError as te:
    print(str(te))  # __traceback__ must be a traceback or None

# Assigning None to __traceback__ is always valid.
e4 = KeyError("k")
e4.__traceback__ = None
print(e4.__traceback__ is None)  # True
