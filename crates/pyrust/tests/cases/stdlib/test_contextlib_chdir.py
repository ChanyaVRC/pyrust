# contextlib.chdir (issue #2802): a reentrant context manager that
# changes the working directory on __enter__ and restores the previous
# one on __exit__ (even on exception).
#
# Portable across the parity harness's three CI OSes.  Windows has no
# `/tmp` (CPython itself raises FileNotFoundError on `chdir("/tmp")`),
# and macOS reaches `/tmp` through a symlink, so this fixture never
# hardcodes a Unix path or string-compares the cwd against one.  It
# picks a writeable temp base the way `tempfile.gettempdir()` does —
# without the (unavailable) `tempfile` module, mirroring
# test_os_fs_ops.py — and asserts the cwd *changes* and *restores*
# rather than equalling a specific absolute path.
import contextlib
import os
import os.path


def _pick_tempdir_base():
    for var in ("TMPDIR", "TEMP", "TMP"):
        candidate = os.environ.get(var)
        if candidate and os.path.isdir(candidate):
            return candidate
    if os.path.isdir("/tmp"):
        return "/tmp"
    return "."


def best_effort_cleanup(path):
    if not os.path.exists(path):
        return
    walk = list(os.walk(path))
    walk.reverse()
    for dirpath, _dirs, files in walk:
        for f in files:
            try:
                os.remove(os.path.join(dirpath, f))
            except OSError:
                pass
    walk = list(os.walk(path))
    walk.reverse()
    for dirpath, _dirs, _files in walk:
        try:
            os.rmdir(dirpath)
        except OSError:
            pass


# Available as an attribute and importable.
print(hasattr(contextlib, "chdir"))  # True
from contextlib import chdir

print(chdir.__name__)  # chdir

ROOT = os.path.join(_pick_tempdir_base(), "pyrust-chdir-test-2802")
SUB = os.path.join(ROOT, "sub")
best_effort_cleanup(ROOT)
os.makedirs(SUB)

original = os.getcwd()

# Basic usage: cwd changes inside the block, restored after.  Compare by
# basename (portable; avoids symlink/path-format differences) plus a
# "changed from original" check.
with contextlib.chdir(ROOT):
    print(os.getcwd() != original)  # True (changed)
    print(os.path.basename(os.getcwd()) == "pyrust-chdir-test-2802")  # True

print(os.getcwd() == original)  # True (restored)

# Exception still restores directory and propagates.
try:
    with contextlib.chdir(ROOT):
        print(os.path.basename(os.getcwd()) == "pyrust-chdir-test-2802")  # True
        raise ValueError("oops")
except ValueError:
    print("caught")  # caught

print(os.getcwd() == original)  # True (restored even after exception)

# Nested usage restores in LIFO order.
with contextlib.chdir(ROOT):
    inner = os.getcwd()
    with contextlib.chdir(SUB):
        print(os.path.basename(os.getcwd()) == "sub")  # True
        print(os.getcwd() != inner)  # True
    print(os.getcwd() == inner)  # True (restored to ROOT)

print(os.getcwd() == original)  # True

# Cleanup — must be outside ROOT before removing it.
os.chdir(original)
best_effort_cleanup(ROOT)

print("contextlib.chdir ok")
