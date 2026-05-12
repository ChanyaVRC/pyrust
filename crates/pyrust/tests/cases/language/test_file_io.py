# open() and file I/O

# Use a unique-ish path so concurrent test runs don't collide.
path = "_pyrust_io_test.tmp"

# --- write mode ---
f = open(path, "w")
n = f.write("hello\n")
assert n == 6
n2 = f.write("world\n")
assert n2 == 6
f.close()

# --- read mode (default 'r') ---
f = open(path)
content = f.read()
assert content == "hello\nworld\n"
f.close()

# --- read with size argument ---
f = open(path)
assert f.read(5) == "hello"
assert f.read(1) == "\n"
assert f.read() == "world\n"
f.close()

# --- readline ---
f = open(path)
assert f.readline() == "hello\n"
assert f.readline() == "world\n"
assert f.readline() == ""    # EOF
f.close()

# --- readlines ---
f = open(path)
lines = f.readlines()
assert lines == ["hello\n", "world\n"]
f.close()

# --- writelines ---
f = open(path, "w")
f.writelines(["a\n", "b\n", "c\n"])
f.close()

f = open(path)
assert f.read() == "a\nb\nc\n"
f.close()

# --- context manager (with statement) ---
with open(path, "w") as f:
    f.write("ctx\n")
# Auto-closed: writes flushed on __exit__
with open(path) as f:
    assert f.read() == "ctx\n"

# --- file is its own iterator ---
with open(path, "w") as f:
    f.writelines(["line1\n", "line2\n", "line3\n"])

with open(path) as f:
    collected = []
    for line in f:
        collected.append(line)
assert collected == ["line1\n", "line2\n", "line3\n"]

# --- append mode ---
with open(path, "w") as f:
    f.write("first\n")
with open(path, "a") as f:
    f.write("second\n")
with open(path) as f:
    assert f.read() == "first\nsecond\n"

# --- error: read on closed file ---
f = open(path)
f.close()
try:
    f.read()
    print("FAIL: expected ValueError")
except ValueError:
    pass

# --- error: file not found ---
try:
    open("nonexistent_file_for_pyrust_test.tmp")
    print("FAIL: expected FileNotFoundError")
except FileNotFoundError:
    pass
except OSError:
    pass

# Cleanup: open(path, "w") followed by no writes truncates the file —
# good enough since "os" isn't available yet to actually remove it.
with open(path, "w") as f:
    pass

print("file io OK")
