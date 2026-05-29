# Parity fixture for issue #1020: EOFError, IndentationError, and TabError
# must be available as builtins and correctly wired into the exception hierarchy.
#
# EOFError         → Exception
# IndentationError → SyntaxError → Exception
# TabError         → IndentationError → SyntaxError → Exception

# --- Availability ---
print(EOFError.__name__)
print(IndentationError.__name__)
print(TabError.__name__)

# --- Hierarchy via issubclass ---
print(issubclass(EOFError, Exception))
print(issubclass(EOFError, BaseException))

print(issubclass(IndentationError, SyntaxError))
print(issubclass(IndentationError, Exception))
print(issubclass(IndentationError, BaseException))

print(issubclass(TabError, IndentationError))
print(issubclass(TabError, SyntaxError))
print(issubclass(TabError, Exception))
print(issubclass(TabError, BaseException))

# EOFError is NOT a subclass of SyntaxError (they are siblings under Exception)
print(issubclass(EOFError, SyntaxError))
print(issubclass(SyntaxError, EOFError))

# --- except Exception catches all three ---
try:
    raise EOFError("end of file")
except Exception as e:
    print(type(e).__name__, e.args[0])

try:
    raise IndentationError("unexpected indent")
except Exception as e:
    print(type(e).__name__, e.args[0])

try:
    raise TabError("inconsistent use of tabs")
except Exception as e:
    print(type(e).__name__, e.args[0])

# --- IndentationError caught by except SyntaxError ---
try:
    raise IndentationError("indent")
except SyntaxError as e:
    print("caught IndentationError as SyntaxError:", type(e).__name__)

# --- TabError caught by except IndentationError ---
try:
    raise TabError("tab")
except IndentationError as e:
    print("caught TabError as IndentationError:", type(e).__name__)

# --- TabError caught by except SyntaxError ---
try:
    raise TabError("tab")
except SyntaxError as e:
    print("caught TabError as SyntaxError:", type(e).__name__)

# --- Instance args are preserved ---
e = EOFError("eof msg")
print(e.args)

ie = IndentationError("indent msg")
print(ie.args)

te = TabError("tab msg")
print(te.args)
