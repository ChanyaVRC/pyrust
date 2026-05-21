# Verify that error messages are correct when exceptions propagate through
# function call chains.  The parity harness strips "Traceback ..." and
# "File ..." lines before diffing, so we only compare the exception class
# and message — not the frame-chain header itself (which varies by path).

def inner():
    raise ValueError("from inner")

def outer():
    inner()

# Error propagated through a call chain — parity tests the message, not
# the traceback header (the harness strips those lines).
try:
    outer()
except ValueError as e:
    print(type(e).__name__, str(e))

# Error at module scope
try:
    raise TypeError("at module scope")
except TypeError as e:
    print(type(e).__name__, str(e))

# Error caught and re-raised (exception chaining not tested here — separate issue)
try:
    try:
        raise RuntimeError("original")
    except RuntimeError:
        raise KeyError("replacement")
except KeyError as e:
    print(type(e).__name__, str(e))

# Verify that catching an exception does NOT pollute the traceback for a
# subsequent error raised at a different site.
def raises_then_catches():
    try:
        raise ValueError("caught internally")
    except ValueError:
        pass

raises_then_catches()

try:
    raise IndexError("fresh error")
except IndexError as e:
    print(type(e).__name__, str(e))

print("done")
