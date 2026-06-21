import contextlib
import io

# nullcontext.__exit__ returns None (not False) on the non-suppressing path.
n = contextlib.nullcontext()
n.__enter__()
print(n.__exit__(None, None, None) is None)  # True
# Even with an exception present, nullcontext returns None unconditionally.
print(n.__exit__(ValueError, ValueError("x"), None) is None)  # True

# closing.__exit__ returns None unconditionally after calling .close().
c = contextlib.closing(io.StringIO())
c.__enter__()
print(c.__exit__(None, None, None) is None)  # True
print(c.__exit__(ValueError, ValueError("x"), None) is None)  # True

# redirect_stdout.__exit__ returns None after restoring the stream.
ctx = contextlib.redirect_stdout(io.StringIO())
ctx.__enter__()
print(ctx.__exit__(None, None, None) is None)  # True

# suppress.__exit__ returns None on the no-exception path.
s = contextlib.suppress(ValueError)
s.__enter__()
print(s.__exit__(None, None, None) is None)  # True

# suppress.__exit__ returns True when it suppresses a matching exception.
s2 = contextlib.suppress(ValueError)
s2.__enter__()
print(s2.__exit__(ValueError, ValueError("x"), None))  # True

# suppress.__exit__ returns False (not None) for a non-matching exception.
s3 = contextlib.suppress(ValueError)
s3.__enter__()
print(s3.__exit__(KeyError, KeyError("y"), None))  # False

# @contextmanager no-exception exit returns False (not None), matching CPython.
@contextlib.contextmanager
def cm():
    yield 1


m = cm()
m.__enter__()
print(m.__exit__(None, None, None))  # False

print("contextlib exit None ok")
