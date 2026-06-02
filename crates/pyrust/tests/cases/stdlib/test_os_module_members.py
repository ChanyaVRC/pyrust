# os module members added for issue #2015:
# constants name/linesep/curdir/pardir/extsep/pathsep/altsep/devnull,
# functions getpid/getppid/cpu_count/urandom/strerror/get_terminal_size/
# fspath/stat.
#
# POSIX constants and fspath are DETERMINISTIC (parity-compared).  The
# process/system functions are environment-specific, so they are asserted
# by TYPE only.

import os

# --- POSIX constants (deterministic) ---
print("name", os.name)
print("linesep", repr(os.linesep))
print("curdir", os.curdir)
print("pardir", os.pardir)
print("extsep", os.extsep)
print("pathsep", os.pathsep)
print("altsep", os.altsep)
print("devnull", os.devnull)

# --- fspath: deterministic ---
print("fspath-str", os.fspath("a/b"))
print("fspath-bytes", os.fspath(b"x"))


class P:
    def __fspath__(self):
        return "/from/fspath"


print("fspath-protocol", os.fspath(P()))

try:
    os.fspath(123)
except TypeError:
    print("fspath-typeerror", True)

# --- environment-specific: type-only assertions ---
print("getpid-int>0", isinstance(os.getpid(), int) and os.getpid() > 0)
print("getppid-int", isinstance(os.getppid(), int))
print("cpu_count-int-or-none", isinstance(os.cpu_count(), (int, type(None))))
print("urandom-bytes-len", isinstance(os.urandom(16), bytes) and len(os.urandom(16)) == 16)
print("strerror-str", isinstance(os.strerror(2), str))
# get_terminal_size() and stat() are exercised manually rather than in this
# fixture: CPython's get_terminal_size() raises OSError when stdout is a
# pipe (as under the parity harness), while a fallback-based implementation
# would not, so the outputs cannot be byte-compared. stat() metadata
# (sizes/inodes) is likewise environment-specific. Both are verified for
# existence/type in the implementer's manual checks.
