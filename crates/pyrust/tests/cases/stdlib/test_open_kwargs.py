"""Parity fixture for open() keyword arguments (issue #1360).

Tests that open() accepts encoding, buffering, errors, newline, and closefd
as keyword arguments without raising TypeError, and that encoding is wired
through for text-mode reads and writes.
"""

import os

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_TMPFILE = "/tmp/_pyrust_test_open_kwargs.txt"


def cleanup():
    try:
        os.unlink(_TMPFILE)
    except OSError:
        pass


# ---------------------------------------------------------------------------
# encoding kwarg accepted and wired through
# ---------------------------------------------------------------------------

# Write with encoding='utf-8' keyword argument.
with open(_TMPFILE, "w", encoding="utf-8") as f:
    f.write("hello world")

# Read back with encoding='utf-8'.
with open(_TMPFILE, "r", encoding="utf-8") as f:
    print(f.read())  # hello world

cleanup()

# ---------------------------------------------------------------------------
# buffering, errors, newline accepted without TypeError
# ---------------------------------------------------------------------------

with open(_TMPFILE, "w") as f:
    f.write("test")

with open(_TMPFILE, "r", buffering=-1) as f:
    pass
print("buffering kwarg ok")

with open(_TMPFILE, "r", errors="strict") as f:
    pass
print("errors kwarg ok")

with open(_TMPFILE, "r", newline=None) as f:
    pass
print("newline kwarg ok")

cleanup()

# ---------------------------------------------------------------------------
# closefd=True is accepted
# ---------------------------------------------------------------------------

with open(_TMPFILE, "w") as f:
    f.write("x")

with open(_TMPFILE, "r", closefd=True) as f:
    pass
print("closefd=True ok")

cleanup()

# ---------------------------------------------------------------------------
# closefd=False with a filename raises ValueError
# ---------------------------------------------------------------------------

with open(_TMPFILE, "w") as f:
    f.write("x")

try:
    open(_TMPFILE, "r", closefd=False)
    print("FAIL: expected ValueError")
except ValueError:
    print("closefd=False ValueError ok")

cleanup()

# ---------------------------------------------------------------------------
# binary mode + encoding raises ValueError
# ---------------------------------------------------------------------------

with open(_TMPFILE, "wb") as f:
    f.write(b"bytes")

try:
    open(_TMPFILE, "rb", encoding="utf-8")
    print("FAIL: expected ValueError")
except ValueError:
    print("binary+encoding ValueError ok")

cleanup()

# ---------------------------------------------------------------------------
# unknown encoding raises LookupError (with an existing file)
# ---------------------------------------------------------------------------

with open(_TMPFILE, "w") as f:
    f.write("x")

try:
    open(_TMPFILE, "r", encoding="no_such_codec_xyz")
    print("FAIL: expected LookupError")
except LookupError:
    print("unknown encoding LookupError ok")

cleanup()

# ---------------------------------------------------------------------------
# ascii encoding for ASCII content
# ---------------------------------------------------------------------------

with open(_TMPFILE, "w", encoding="ascii") as f:
    f.write("pure ascii text")

with open(_TMPFILE, "r", encoding="ascii") as f:
    print(f.read())  # pure ascii text

cleanup()

# ---------------------------------------------------------------------------
# latin-1 encoding
# ---------------------------------------------------------------------------

with open(_TMPFILE, "wb") as f:
    f.write(b"\xe9\xe0\xfc")  # é, à, ü as latin-1

with open(_TMPFILE, "r", encoding="latin-1") as f:
    s = f.read()
    print(len(s), ord(s[0]), ord(s[1]), ord(s[2]))  # 3 233 224 252

cleanup()

# ---------------------------------------------------------------------------
# .encoding attribute preserves user-supplied case (CPython verbatim passthrough)
# ---------------------------------------------------------------------------

with open(_TMPFILE, "w") as f:
    f.write("x")

f = open(_TMPFILE, "w", encoding="utf-8")
print(f.encoding)   # utf-8 (not UTF-8)
f.close()

f = open(_TMPFILE, "w", encoding="Utf-8")
print(f.encoding)   # Utf-8
f.close()

f = open(_TMPFILE, "w", encoding="latin-1")
print(f.encoding)   # latin-1
f.close()

cleanup()
