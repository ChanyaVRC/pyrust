# Issue #2600: contextlib.redirect_stdout / redirect_stderr context managers.
import contextlib
import io
import sys

# redirect_stdout captures print() into a StringIO.
buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    print("hello")
    print("world", end="!")
print("captured:", repr(buf.getvalue()))

# stdout is restored after the block.
print("restored")

# redirect_stderr captures sys.stderr.write().
errbuf = io.StringIO()
with contextlib.redirect_stderr(errbuf):
    sys.stderr.write("err line\n")
print("stderr captured:", repr(errbuf.getvalue()))

# from-import form, and the with-target binds to new_target.
from contextlib import redirect_stdout, redirect_stderr

b = io.StringIO()
with redirect_stdout(b) as target:
    print("via from-import")
print("target is buffer:", target is b)
print(repr(b.getvalue()))

# Nested redirects restore in LIFO order.
outer = io.StringIO()
inner = io.StringIO()
with redirect_stdout(outer):
    print("outer-1")
    with redirect_stdout(inner):
        print("inner-1")
    print("outer-2")
print("outer:", repr(outer.getvalue()))
print("inner:", repr(inner.getvalue()))

# Exception inside the block still restores the stream.
ebuf = io.StringIO()
try:
    with contextlib.redirect_stdout(ebuf):
        print("before-raise")
        raise ValueError("boom")
except ValueError:
    pass
print("exc captured:", repr(ebuf.getvalue()))
print("stdout restored after exception")

# A redirect_stdout instance is reusable across multiple with-blocks.
reuse_buf = io.StringIO()
cm = contextlib.redirect_stdout(reuse_buf)
with cm:
    print("first")
with cm:
    print("second")
print("reused:", repr(reuse_buf.getvalue()))
