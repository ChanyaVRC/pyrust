"""CPython 3.12.3 parity tests for real ctypes FFI calls, callbacks, and library loading."""

import ctypes
import ctypes.util
import math
import os
import sys

# Load standard C / math libraries on the active OS
if sys.platform == "win32":
    libc = ctypes.CDLL("msvcrt")
    libm = libc
    kernel32 = ctypes.WinDLL("kernel32")
elif sys.platform == "darwin":
    libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
    libm = libc
else:
    libc_name = ctypes.util.find_library("c")
    libm_name = ctypes.util.find_library("m")
    assert libc_name is not None
    assert libm_name is not None
    libc = ctypes.CDLL(libc_name)
    libm = ctypes.CDLL(libm_name)

# 1. Integer and string FFI calls: abs, strlen, strcmp, memcmp
libc.abs.argtypes = [ctypes.c_int]
libc.abs.restype = ctypes.c_int
assert libc.abs(-42) == 42
assert libc.abs(0) == 0
assert libc.abs(12345) == 12345

libc.strlen.argtypes = [ctypes.c_char_p]
libc.strlen.restype = ctypes.c_size_t
assert libc.strlen(b"") == 0
assert libc.strlen(b"hello world") == 11
sbuf = ctypes.create_string_buffer(b"pyrust ffi")
assert libc.strlen(sbuf) == 10

libc.strcmp.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
libc.strcmp.restype = ctypes.c_int
assert libc.strcmp(b"abc", b"abc") == 0
assert libc.strcmp(b"abc", b"abd") < 0
assert libc.strcmp(b"abd", b"abc") > 0

libc.atoi.argtypes = [ctypes.c_char_p]
libc.atoi.restype = ctypes.c_int
assert libc.atoi(b"-9876") == -9876
print("int and string FFI ok")

# 2. Floating-point FFI calls: sin, cos, pow, sqrt, fabs
libm.sin.argtypes = [ctypes.c_double]
libm.sin.restype = ctypes.c_double
libm.cos.argtypes = [ctypes.c_double]
libm.cos.restype = ctypes.c_double
libm.pow.argtypes = [ctypes.c_double, ctypes.c_double]
libm.pow.restype = ctypes.c_double
libm.sqrt.argtypes = [ctypes.c_double]
libm.sqrt.restype = ctypes.c_double
libm.fabs.argtypes = [ctypes.c_double]
libm.fabs.restype = ctypes.c_double

assert abs(libm.sin(0.0) - 0.0) < 1e-12
assert abs(libm.cos(0.0) - 1.0) < 1e-12
assert abs(libm.sin(math.pi / 2) - 1.0) < 1e-12
assert abs(libm.pow(2.0, 10.0) - 1024.0) < 1e-12
assert abs(libm.sqrt(81.0) - 9.0) < 1e-12
assert abs(libm.fabs(-3.5) - 3.5) < 1e-12
print("float FFI ok")

# 3. `qsort` with a `CFUNCTYPE` Python callback comparator!
CMPFUNC = ctypes.CFUNCTYPE(
    ctypes.c_int,
    ctypes.POINTER(ctypes.c_int),
    ctypes.POINTER(ctypes.c_int),
)

call_count = 0


def py_cmp(a, b):
    global call_count
    call_count += 1
    va = a[0]
    vb = b[0]
    if va < vb:
        return -1
    if va > vb:
        return 1
    return 0


cmp_cb = CMPFUNC(py_cmp)

# Direct invocation of CFUNCTYPE callback from Python
x1 = ctypes.c_int(5)
x2 = ctypes.c_int(9)
assert cmp_cb(ctypes.pointer(x1), ctypes.pointer(x2)) == -1
assert cmp_cb(ctypes.pointer(x2), ctypes.pointer(x1)) == 1
assert cmp_cb(ctypes.pointer(x1), ctypes.pointer(x1)) == 0

# Real C qsort calling back into Python!
IntArr = ctypes.c_int * 7
nums = IntArr(42, -5, 17, 0, 99, 3, -100)
libc.qsort.argtypes = [
    ctypes.c_void_p,
    ctypes.c_size_t,
    ctypes.c_size_t,
    CMPFUNC,
]
libc.qsort.restype = None
libc.qsort(nums, len(nums), ctypes.sizeof(ctypes.c_int), cmp_cb)
assert list(nums) == [-100, -5, 0, 3, 17, 42, 99]
assert call_count > 3
print("qsort callback ok:", list(nums))

# 4. errcheck hook
seen_errcheck = []


def check_abs(result, func, args):
    seen_errcheck.append((result, args))
    return result * 10


abs_fn = libc["abs"]
abs_fn.argtypes = [ctypes.c_int]
abs_fn.restype = ctypes.c_int
abs_fn.errcheck = check_abs
assert abs_fn(-7) == 70
assert seen_errcheck == [(7, (-7,))]
print("errcheck ok")

# 5. errno get/set and use_errno
old = ctypes.set_errno(12)
assert isinstance(old, int)
assert ctypes.get_errno() == 12
assert ctypes.set_errno(0) == 12
assert ctypes.get_errno() == 0

if sys.platform == "win32":
    libc_e = ctypes.CDLL("ucrtbase", use_errno=True)
elif sys.platform == "darwin":
    libc_e = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
else:
    libc_e = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)

libc_e.strtol.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_int]
libc_e.strtol.restype = ctypes.c_long
ctypes.set_errno(0)
assert libc_e.strtol(b"12345", None, 10) == 12345
assert ctypes.get_errno() == 0
# Huge overflow triggers ERANGE
_ = libc_e.strtol(b"999999999999999999999999999999999999999", None, 10)
assert ctypes.get_errno() != 0
ctypes.set_errno(0)
print("errno ok")

# 6. Platform-specific library discovery and Windows APIs
assert ctypes.util.find_library("definitely_missing_lib_pyrust_xyz") is None

if sys.platform == "win32":
    k32_path = ctypes.util.find_library("kernel32")
    assert k32_path is not None and k32_path.lower().endswith("kernel32.dll")
    kernel32.GetCurrentProcessId.argtypes = []
    kernel32.GetCurrentProcessId.restype = ctypes.c_uint32
    assert kernel32.GetCurrentProcessId() == os.getpid()

    # Windows last_error tracking
    prev_le = ctypes.set_last_error(5)
    assert isinstance(prev_le, int)
    assert ctypes.get_last_error() == 5
    assert ctypes.set_last_error(0) == 5
    assert isinstance(ctypes.GetLastError(), int)
    assert isinstance(ctypes.FormatError(2), str) and len(ctypes.FormatError(2)) > 0
    we = ctypes.WinError(2)
    assert isinstance(we, OSError)
    print("windows FFI ok")
else:
    libc.getpid.argtypes = []
    libc.getpid.restype = ctypes.c_int
    assert libc.getpid() == os.getpid()
    print("unix FFI ok")

print("ALL ctypes FFI parity tests passed!")
