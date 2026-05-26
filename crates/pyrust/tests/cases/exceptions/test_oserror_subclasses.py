# CPython 3.12: OSError subclasses must be available as builtins and form the
# correct inheritance chain.

# Direct OSError subclasses
print(BlockingIOError("would block"))
print(ChildProcessError("child"))
print(FileExistsError("exists"))
print(FileNotFoundError("not found"))
print(InterruptedError("interrupted"))
print(IsADirectoryError("is dir"))
print(NotADirectoryError("not dir"))
print(PermissionError("denied"))
print(ProcessLookupError("lookup"))
print(TimeoutError("timed out"))

# ConnectionError and its subclasses
print(ConnectionError("connection"))
print(BrokenPipeError("broken pipe"))
print(ConnectionAbortedError("aborted"))
print(ConnectionRefusedError("refused"))
print(ConnectionResetError("reset"))

# SyntaxError subclasses
print(IndentationError("bad indent"))
print(TabError("bad tab"))

# isinstance checks — direct OSError subclasses
print(isinstance(BlockingIOError(), OSError))
print(isinstance(ChildProcessError(), OSError))
print(isinstance(FileExistsError(), OSError))
print(isinstance(FileNotFoundError(), OSError))
print(isinstance(InterruptedError(), OSError))
print(isinstance(IsADirectoryError(), OSError))
print(isinstance(NotADirectoryError(), OSError))
print(isinstance(PermissionError(), OSError))
print(isinstance(ProcessLookupError(), OSError))
print(isinstance(TimeoutError(), OSError))

# ConnectionError hierarchy
print(isinstance(ConnectionError(), OSError))
print(isinstance(BrokenPipeError(), ConnectionError))
print(isinstance(BrokenPipeError(), OSError))
print(isinstance(ConnectionAbortedError(), ConnectionError))
print(isinstance(ConnectionRefusedError(), ConnectionError))
print(isinstance(ConnectionResetError(), ConnectionError))

# issubclass checks
print(issubclass(BrokenPipeError, ConnectionError))
print(issubclass(ConnectionError, OSError))
print(issubclass(PermissionError, Exception))

# SyntaxError subclasses
print(isinstance(IndentationError(), SyntaxError))
print(isinstance(TabError(), IndentationError))
print(isinstance(TabError(), SyntaxError))
print(issubclass(TabError, IndentationError))

# except clause catches via parent
try:
    raise PermissionError("denied")
except OSError as e:
    print("caught OSError:", e)

try:
    raise BrokenPipeError("broken")
except ConnectionError as e:
    print("caught ConnectionError:", e)

try:
    raise BrokenPipeError("broken")
except OSError as e:
    print("caught OSError from BrokenPipeError:", e)

try:
    raise TabError("bad tab")
except IndentationError as e:
    print("caught IndentationError:", e)

try:
    raise TabError("bad tab")
except SyntaxError as e:
    print("caught SyntaxError:", e)

# OSError structured attributes
e = PermissionError(13, "Permission denied", "/etc/shadow")
print(e.errno)
print(e.strerror)
print(e.filename)

# IOError and EnvironmentError are still OSError aliases
print(IOError is OSError)
print(EnvironmentError is OSError)
