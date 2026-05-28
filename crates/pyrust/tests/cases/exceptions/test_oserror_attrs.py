# OSError structured attributes: errno, strerror, filename, filename2.
#
# CPython 3.12 sets these on every OSError (and subclass) raised from a real
# OS operation, and also when the class is constructed with the 2- or 3-arg
# forms.  This fixture exercises both the OS-raised path and manual
# construction.

import os, sys

# ── OS-raised path: FileNotFoundError from open() ───────────────────────────

try:
    open("/no_such_file_pyrust_test_abc123", "r")
except FileNotFoundError as e:
    print(type(e).__name__)
    print(e.errno)
    # strerror text is platform-specific for OS-raised errors.
    print(e.filename)

# ── OS-raised path: FileNotFoundError from os.remove() ──────────────────────

try:
    os.remove("/no_such_file_pyrust_test_remove")
except FileNotFoundError as e:
    print(type(e).__name__)
    print(e.errno)
    # strerror is platform-specific for OS-raised errors.
    print(e.filename)

# ── OS-raised path: FileNotFoundError from os.rmdir() ───────────────────────

try:
    os.rmdir("/no_such_dir_pyrust_test")
except FileNotFoundError as e:
    print(type(e).__name__)
    print(e.errno)
    # strerror is platform-specific for OS-raised errors.
    print(e.filename)

# ── Manual construction: 3-arg form ─────────────────────────────────────────

e = OSError(2, "No such file or directory", "myfile.txt")
print(e.errno)
print(e.strerror)
print(e.filename)
print(e.filename2)
# args is (errno, strerror) only — filename is NOT included (CPython 3.12 behaviour)
print(e.args)

# ── Manual construction: 2-arg form ─────────────────────────────────────────

e = OSError(13, "Permission denied")
print(e.errno)
print(e.strerror)
print(e.filename)
print(e.args)

# ── Manual construction: 1-arg form — errno/strerror are None ───────────────

e = OSError(42)
print(e.errno)
print(e.strerror)
print(e.filename)
print(e.args)

# ── Manual construction: 5-arg form — filename2 is set ──────────────────────

e = OSError(2, "No such file or directory", "src.txt", None, "dst.txt")
print(e.errno)
print(e.strerror)
print(e.filename)
print(e.filename2)

# ── str() and repr() format ─────────────────────────────────────────────────

try:
    open("/no_such_file_str_test_abc", "r")
except FileNotFoundError as e:
    # str(e) produces "[Errno N] message: 'filename'"
    s = str(e)
    print(s.startswith("[Errno 2]"))
    # strerror is platform-specific; only check the stable parts.
    print("/no_such_file_str_test_abc" in s)
    # Cross-platform: strerror must appear in str(e) regardless of OS wording.
    print(e.strerror in s)

# repr() of a 3-arg FileNotFoundError shows only (errno, strerror) — no filename
e = FileNotFoundError(2, "No such file or directory", "/path")
print(repr(e))
