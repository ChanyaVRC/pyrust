# CPython 3.12: five exception classes that were previously missing from pyrust.

# --- BufferError ---
print(issubclass(BufferError, Exception))   # True
print(issubclass(BufferError, BaseException))  # True
e = BufferError("buf err")
print(e.args)  # ('buf err',)
try:
    raise BufferError("buffer gone")
except BufferError as exc:
    print(str(exc))  # buffer gone
try:
    raise BufferError("buffer gone")
except Exception as exc:
    print(type(exc).__name__)  # BufferError

# --- ReferenceError ---
print(issubclass(ReferenceError, Exception))   # True
print(issubclass(ReferenceError, BaseException))  # True
e = ReferenceError("dead weakref")
print(e.args)  # ('dead weakref',)
try:
    raise ReferenceError("weak ref gone")
except ReferenceError as exc:
    print(str(exc))  # weak ref gone

# --- SystemError ---
print(issubclass(SystemError, Exception))   # True
print(issubclass(SystemError, BaseException))  # True
e = SystemError("internal")
print(e.args)  # ('internal',)
try:
    raise SystemError("bad internal call")
except SystemError as exc:
    print(str(exc))  # bad internal call

# --- StopAsyncIteration ---
print(issubclass(StopAsyncIteration, Exception))   # True
print(issubclass(StopAsyncIteration, BaseException))  # True
# StopAsyncIteration is NOT a subclass of StopIteration
print(issubclass(StopAsyncIteration, StopIteration))  # False
e = StopAsyncIteration()
print(e.args)  # ()
try:
    raise StopAsyncIteration
except StopAsyncIteration:
    print("caught StopAsyncIteration")  # caught StopAsyncIteration

# --- UnicodeTranslateError ---
print(issubclass(UnicodeTranslateError, UnicodeError))   # True
print(issubclass(UnicodeTranslateError, ValueError))   # True
print(issubclass(UnicodeTranslateError, Exception))   # True
e = UnicodeTranslateError("x", 0, 1, "reason")
print(e.args)  # ('x', 0, 1, 'reason')
try:
    raise UnicodeTranslateError("abc", 1, 2, "bad char")
except UnicodeError as exc:
    print(type(exc).__name__)  # UnicodeTranslateError
