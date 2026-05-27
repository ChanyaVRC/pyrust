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
print(f.encoding)        # UTF-8
print(f.tell())          # 0
n = f.write("hello\n")
print(n)                 # 6
print(f.tell())          # 6
n = f.write("world\n")
print(n)                 # 6
print(f.tell())          # 12
f.flush()
print(f.closed)          # False
f.close()
print(f.closed)          # True

# --- read mode ---
f = open(FNAME, "r")
print(f.name == FNAME)   # True
print(f.mode)            # r
print(f.closed)          # False
print(f.encoding)        # UTF-8
print(f.tell())          # 0
line = f.readline()
print(repr(line))        # 'hello\n'
print(f.tell())          # 6
lines = f.readlines()
print(lines)             # ['world\n']
print(f.tell())          # 12
f.close()
print(f.closed)          # True

# --- seek + tell ---
f = open(FNAME, "r")
print(f.tell())          # 0
f.seek(6)
print(f.tell())          # 6
print(repr(f.read()))    # 'world\n'
f.seek(0)
print(f.tell())          # 0
print(repr(f.readline()))  # 'hello\n'
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
