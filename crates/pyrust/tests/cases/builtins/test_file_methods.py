"""Parity fixture: file object methods and properties (issue #1265).

Tests tell(), seek(), readline(), readlines(), flush(), and the .name,
.mode, .closed, .encoding attributes on file objects.
"""

import os

FNAME = "pyrust_test_file_methods.txt"

# --- write mode ---
f = open(FNAME, "w")
print(f.name == FNAME)   # True
print(f.mode)            # w
print(f.closed)          # False
n = f.write("hello\n")
print(n)                 # 6
n = f.write("world\n")
print(n)                 # 6
f.flush()
print(f.closed)          # False
f.close()
print(f.closed)          # True

# --- read mode ---
f = open(FNAME, "r")
print(f.name == FNAME)   # True
print(f.mode)            # r
print(f.closed)          # False
line = f.readline()
print(repr(line))        # 'hello\n'
lines = f.readlines()
print(lines)             # ['world\n']
f.close()
print(f.closed)          # True

# --- seek + tell (binary mode for cross-platform byte consistency) ---
with open(FNAME, "wb") as f:
    f.write(b"hello\nworld\n")  # exactly 12 bytes on all platforms

f = open(FNAME, "rb")
print(f.tell())          # 0
f.seek(6)
print(f.tell())          # 6
print(repr(f.read()))    # b'world\n'
f.seek(0)
print(f.tell())          # 0
print(repr(f.readline()))  # b'hello\n'
# seek(0, 2) goes to end
f.seek(0, 2)
print(f.tell())          # 12
# seek(0, 1) stays at current
pos = f.seek(0, 1)
print(pos)               # 12
f.close()

# --- encoding absent on binary mode ---
f = open(FNAME, "rb")
print(hasattr(f, "encoding"))  # False
f.close()

# --- closed on already-closed file ---
f = open(FNAME, "r")
f.close()
print(f.closed)   # True
print(f.name == FNAME)  # True (accessible after close)

# cleanup
os.remove(FNAME)
