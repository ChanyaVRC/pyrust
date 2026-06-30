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


# @contextmanager re-raising the *same* exception (or not catching it) is a
# non-suppressing exit: __exit__ returns False rather than re-raising, so a
# direct call observes False.
@contextlib.contextmanager
def cm_reraise():
    try:
        yield
    except ValueError:
        raise


r = cm_reraise()
r.__enter__()
print(r.__exit__(ValueError, ValueError("x"), None))  # False


@contextlib.contextmanager
def cm_nocatch():
    yield


nc = cm_nocatch()
nc.__enter__()
print(nc.__exit__(ValueError, ValueError("x"), None))  # False


# Same non-suppressing path with exc_val=None: CPython materialises typ() and
# returns False when the generator lets it propagate.
nc2 = cm_nocatch()
nc2.__enter__()
print(nc2.__exit__(ValueError, None, None))  # False


# @contextmanager swallowing a matching exception suppresses it (returns True).
@contextlib.contextmanager
def cm_swallow():
    try:
        yield
    except ValueError:
        pass


sw = cm_swallow()
sw.__enter__()
print(sw.__exit__(ValueError, ValueError("x"), None))  # True


# @contextmanager raising a *different* exception propagates it (not suppressed).
@contextlib.contextmanager
def cm_transform():
    try:
        yield
    except ValueError:
        raise KeyError("t")


t = cm_transform()
t.__enter__()
try:
    t.__exit__(ValueError, ValueError("x"), None)
    print("no raise")
except KeyError:
    print("transform propagated KeyError")

print("contextlib exit None ok")
