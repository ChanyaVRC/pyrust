# CPython 3.12: OSError(errno, strerror) remaps to an errno-specific subclass
# via OSError.__new__'s _Py_errnomap logic.  Only the plain OSError constructor
# triggers remapping; calling a subclass directly leaves the class unchanged.

# --- Basic remapping ---
print(type(OSError(2, "No such file or directory")).__name__)   # FileNotFoundError
print(type(OSError(13, "Permission denied")).__name__)          # PermissionError
print(type(OSError(17, "File exists")).__name__)                # FileExistsError

# --- Full errno mapping table (Linux values) ---
print(type(OSError(1, "x")).__name__)    # PermissionError   (EPERM)
print(type(OSError(2, "x")).__name__)    # FileNotFoundError (ENOENT)
print(type(OSError(3, "x")).__name__)    # ProcessLookupError (ESRCH)
print(type(OSError(4, "x")).__name__)    # InterruptedError  (EINTR)
print(type(OSError(10, "x")).__name__)   # ChildProcessError (ECHILD)
print(type(OSError(11, "x")).__name__)   # BlockingIOError   (EAGAIN)
print(type(OSError(13, "x")).__name__)   # PermissionError   (EACCES)
print(type(OSError(17, "x")).__name__)   # FileExistsError   (EEXIST)
print(type(OSError(20, "x")).__name__)   # NotADirectoryError (ENOTDIR)
print(type(OSError(21, "x")).__name__)   # IsADirectoryError (EISDIR)
print(type(OSError(32, "x")).__name__)   # BrokenPipeError   (EPIPE)
print(type(OSError(103, "x")).__name__)  # ConnectionAbortedError (ECONNABORTED)
print(type(OSError(104, "x")).__name__)  # ConnectionResetError   (ECONNRESET)
print(type(OSError(108, "x")).__name__)  # BrokenPipeError        (ESHUTDOWN)
print(type(OSError(110, "x")).__name__)  # TimeoutError           (ETIMEDOUT)
print(type(OSError(111, "x")).__name__)  # ConnectionRefusedError (ECONNREFUSED)
print(type(OSError(114, "x")).__name__)  # BlockingIOError        (EALREADY)
print(type(OSError(115, "x")).__name__)  # BlockingIOError        (EINPROGRESS)

# --- errno values with no mapping stay as OSError ---
print(type(OSError(0, "x")).__name__)    # OSError
print(type(OSError(24, "x")).__name__)   # OSError (EMFILE)

# --- Single-arg form is NOT remapped (CPython parity) ---
print(type(OSError(2)).__name__)         # OSError

# --- No args or string arg: no remapping ---
print(type(OSError()).__name__)          # OSError
print(type(OSError("msg")).__name__)     # OSError

# --- isinstance and issubclass checks on remapped instance ---
e = OSError(2, "No such file or directory")
print(isinstance(e, OSError))            # True
print(isinstance(e, FileNotFoundError))  # True

e2 = OSError(13, "Permission denied")
print(isinstance(e2, OSError))           # True
print(isinstance(e2, PermissionError))   # True

# --- Structured attributes are correct on remapped instance ---
e3 = OSError(2, "No such file or directory")
print(e3.errno)                          # 2
print(e3.strerror)                       # No such file or directory

# --- 3-arg form also remaps ---
e4 = OSError(2, "No such file", "/path/to/file")
print(type(e4).__name__)                 # FileNotFoundError
print(e4.errno)                          # 2
print(e4.strerror)                       # No such file
print(e4.filename)                       # /path/to/file

# --- Subclass constructor is NOT remapped ---
# FileNotFoundError(13, ...) stays FileNotFoundError even though 13 -> PermissionError
print(type(FileNotFoundError(13, "x")).__name__)   # FileNotFoundError
print(type(PermissionError(2, "x")).__name__)      # PermissionError

# --- except clause catches remapped instance via parent ---
try:
    raise OSError(2, "not found")
except FileNotFoundError as e:
    print("caught FileNotFoundError:", e.errno)

try:
    raise OSError(32, "broken pipe")
except BrokenPipeError as e:
    print("caught BrokenPipeError:", e.errno)
