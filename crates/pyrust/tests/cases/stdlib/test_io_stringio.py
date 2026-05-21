# io.StringIO and io.BytesIO — in-memory stream tests

import io

# ── StringIO ──────────────────────────────────────────────────────────────────

sio = io.StringIO()
sio.write("hello")
sio.write(" world")
assert sio.getvalue() == "hello world"

# seek + read
sio.seek(0)
assert sio.read() == "hello world"

# seek to middle
sio.seek(6)
assert sio.read() == "world"

# read(n)
sio.seek(0)
assert sio.read(5) == "hello"

# tell
sio.seek(3)
assert sio.tell() == 3

# readline
sio2 = io.StringIO("line1\nline2\nline3")
assert sio2.readline() == "line1\n"
assert sio2.readline() == "line2\n"
assert sio2.readline() == "line3"
assert sio2.readline() == ""  # EOF

# readlines
sio3 = io.StringIO("a\nb\nc\n")
assert sio3.readlines() == ["a\n", "b\n", "c\n"]

# initial value
sio4 = io.StringIO("initial")
assert sio4.getvalue() == "initial"
sio4.seek(0, 2)  # SEEK_END
sio4.write(" more")
assert sio4.getvalue() == "initial more"

# context manager
with io.StringIO() as s:
    s.write("ctx")
    assert s.getvalue() == "ctx"

# close
sio5 = io.StringIO()
sio5.close()
try:
    sio5.read()
    print("FAIL: expected ValueError")
except ValueError:
    pass

# truncate
sio6 = io.StringIO("hello world")
sio6.truncate(5)
assert sio6.getvalue() == "hello"

print("io.StringIO ok")

# ── BytesIO ───────────────────────────────────────────────────────────────────

bio = io.BytesIO()
bio.write(b"hello")
bio.write(b" bytes")
assert bio.getvalue() == b"hello bytes"

bio.seek(0)
assert bio.read() == b"hello bytes"

bio.seek(0)
assert bio.read(5) == b"hello"

# tell
bio.seek(3)
assert bio.tell() == 3

# readline
bio2 = io.BytesIO(b"line1\nline2\n")
assert bio2.readline() == b"line1\n"
assert bio2.readline() == b"line2\n"
assert bio2.readline() == b""

# readlines
bio3 = io.BytesIO(b"a\nb\n")
assert bio3.readlines() == [b"a\n", b"b\n"]

# context manager
with io.BytesIO() as b:
    b.write(b"ctx")
    assert b.getvalue() == b"ctx"

# close
bio5 = io.BytesIO()
bio5.close()
try:
    bio5.read()
    print("FAIL: expected ValueError")
except ValueError:
    pass

print("io.BytesIO ok")
