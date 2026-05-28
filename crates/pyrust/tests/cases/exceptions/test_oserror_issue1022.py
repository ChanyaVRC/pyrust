# CPython 3.12: all OSError subclasses and aliases listed in issue #1022 must be
# accessible as builtins and form the correct inheritance chain.

# Basic availability — issubclass checks
print(issubclass(FileNotFoundError, OSError))
print(issubclass(PermissionError, OSError))
print(issubclass(TimeoutError, OSError))
print(issubclass(ConnectionError, OSError))
print(issubclass(BrokenPipeError, ConnectionError))
print(issubclass(ConnectionAbortedError, ConnectionError))
print(issubclass(ConnectionRefusedError, ConnectionError))
print(issubclass(ConnectionResetError, ConnectionError))
print(issubclass(FileExistsError, OSError))
print(issubclass(IsADirectoryError, OSError))
print(issubclass(NotADirectoryError, OSError))
print(issubclass(BlockingIOError, OSError))
print(issubclass(ChildProcessError, OSError))
print(issubclass(InterruptedError, OSError))
print(issubclass(ProcessLookupError, OSError))

# Aliases — must be the exact same object as OSError
print(IOError is OSError)
print(EnvironmentError is OSError)

# Construction works for direct subclasses
e = PermissionError("no access")
print(type(e).__name__)
print(str(e))

# 2-arg form (non-errno args should still work)
e = TimeoutError(60, "timed out")
print(type(e).__name__)

# isinstance checks including negative cross-class case
e = FileNotFoundError(2, "No such file or directory", "/tmp/x")
print(isinstance(e, OSError))
print(isinstance(e, FileNotFoundError))
print(isinstance(e, PermissionError))

# BrokenPipeError is also an OSError
print(issubclass(BrokenPipeError, OSError))
