"""Parity tests for binary file I/O modes ('rb', 'wb', 'ab')."""
import os

TMP = "_pyrust_test_file_binary.bin"
TMP2 = "_pyrust_test_file_binary2.bin"

# ── binary read ───────────────────────────────────────────────────────────────

# Write a text file then read it back in binary mode.
with open(TMP, "w") as f:
    f.write("hello world")

with open(TMP, "rb") as f:
    data = f.read()
    print(type(data).__name__)   # bytes
    print(repr(data))            # b'hello world'

# Partial binary read
with open(TMP, "rb") as f:
    chunk = f.read(5)
    print(repr(chunk))           # b'hello'
    rest = f.read()
    print(repr(rest))            # b' world'

# ── binary readline / readlines ───────────────────────────────────────────────

with open(TMP, "wb") as f:
    f.write(b"line1\nline2\nline3")

with open(TMP, "rb") as f:
    line = f.readline()
    print(type(line).__name__)   # bytes
    print(repr(line))            # b'line1\n'

with open(TMP, "rb") as f:
    lines = f.readlines()
    print(type(lines[0]).__name__)  # bytes
    print(lines)                    # [b'line1\n', b'line2\n', b'line3']

# ── binary iteration ─────────────────────────────────────────────────────────

with open(TMP, "rb") as f:
    for line in f:
        print(repr(line))

# ── binary write ──────────────────────────────────────────────────────────────

with open(TMP2, "wb") as f:
    n = f.write(b"binary data")
    print(n)                     # 11

with open(TMP2, "rb") as f:
    print(repr(f.read()))        # b'binary data'

# ── binary append ─────────────────────────────────────────────────────────────

with open(TMP2, "wb") as f:
    f.write(b"first")

with open(TMP2, "ab") as f:
    f.write(b" second")

with open(TMP2, "rb") as f:
    print(repr(f.read()))        # b'first second'

# ── binary writelines ─────────────────────────────────────────────────────────

with open(TMP2, "wb") as f:
    f.writelines([b"aaa\n", b"bbb\n"])

with open(TMP2, "rb") as f:
    print(repr(f.read()))        # b'aaa\nbbb\n'

# ── decode ────────────────────────────────────────────────────────────────────

with open(TMP, "rb") as f:
    data = f.read()
    print(data.decode("utf-8"))  # line1\nline2\nline3 (printed as text)

# ── type errors: binary mode rejects str ─────────────────────────────────────

try:
    with open(TMP2, "wb") as f:
        f.write("text")
except TypeError as e:
    print(repr(e))

try:
    with open(TMP2, "wb") as f:
        f.writelines(["line\n"])
except TypeError as e:
    print(repr(e))

# ── type errors: text mode rejects bytes ─────────────────────────────────────

try:
    with open(TMP2, "w") as f:
        f.write(b"bytes")
except TypeError as e:
    print(repr(e))

# ── text mode still works ────────────────────────────────────────────────────

with open(TMP, "w") as f:
    n = f.write("hello")
    print(n)                     # 5

with open(TMP, "r") as f:
    print(type(f.read()).__name__)  # str

# ── cleanup ──────────────────────────────────────────────────────────────────

os.unlink(TMP)
os.unlink(TMP2)
