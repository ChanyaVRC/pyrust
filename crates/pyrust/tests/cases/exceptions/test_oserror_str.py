# OSError.__str__ formats as [Errno N] strerror (or [Errno N] strerror: repr(filename))
# when the 2- or 3-arg constructor is used.  1-arg and 0-arg forms use the default.
# All OSError subclasses inherit this behaviour.

# 2-arg: [Errno N] strerror
print(str(OSError(2, "No such file or directory")))

# 3-arg: [Errno N] strerror: repr(filename)
print(str(OSError(2, "No such file or directory", "/etc/missing")))

# 1-arg: just the message string
print(str(OSError("simple")))

# 0-arg: empty string
print(repr(str(OSError())))

# Subclasses inherit the formatting
print(str(FileNotFoundError(2, "Not found", "/tmp/x")))
print(str(PermissionError(13, "Permission denied")))

# Filename with single quotes uses double-quote repr
print(str(OSError(2, "No such file or directory", "path'with'quotes")))

import sys

# 5-arg form with winerror: output differs on Windows ([WinError N] vs [Errno N])
# Only test on non-Windows where CPython uses [Errno N] format like pyrust.
if sys.platform != "win32":
    # Both filenames set: "[Errno N] strerror: repr(filename) -> repr(filename2)"
    print(str(OSError(2, "Not found", "/src", 0, "/dst")))
    # Only filename2, filename=None: no filename in output
    print(str(OSError(2, "Not found", None, 0, "/dst")))
    # filename set, filename2=None: just filename
    print(str(OSError(2, "Not found", "/path", 0, None)))
