# PEP 654: ExceptionGroup and BaseExceptionGroup (issues #1442, #1114, #1025)

# Basic ExceptionGroup construction
eg = ExceptionGroup("test", [ValueError(1), TypeError(2)])
print(eg.message)           # test
print(eg.exceptions)        # (ValueError(1), TypeError(2))
print(type(eg).__name__)    # ExceptionGroup
print(isinstance(eg, Exception))    # True
print(isinstance(eg, BaseException))  # True

# BaseExceptionGroup with non-Exception members
beg = BaseExceptionGroup("base", [KeyboardInterrupt()])
print(beg.message)          # base
print(type(beg).__name__)   # BaseExceptionGroup
print(isinstance(beg, BaseException))   # True
print(isinstance(beg, Exception))       # False

# BaseExceptionGroup with all-Exception members → promotes to ExceptionGroup
promoted = BaseExceptionGroup("promoted", [ValueError(1)])
print(type(promoted).__name__)   # ExceptionGroup

# except* syntax: single handler
try:
    raise ExceptionGroup("eg", [ValueError(1)])
except* ValueError as eg:
    print("caught ValueError:", eg.exceptions)   # (ValueError(1),)

# except* syntax: multiple handlers on same group
try:
    raise ExceptionGroup("multi", [ValueError(1), TypeError(2)])
except* ValueError as eg:
    print("ValueError handler:", eg.exceptions)  # (ValueError(1),)
except* TypeError as eg:
    print("TypeError handler:", eg.exceptions)   # (TypeError(2),)

# except* with no match re-raises
try:
    try:
        raise ExceptionGroup("eg", [ValueError(1)])
    except* TypeError as eg:
        print("should not reach")
except ExceptionGroup as e:
    print("unmatched re-raised:", e.message)   # eg

# ExceptionGroup validation: message must be str
try:
    ExceptionGroup(42, [ValueError()])
except TypeError as e:
    print("message must be str:", type(e).__name__)

# ExceptionGroup validation: exceptions must be non-empty
try:
    ExceptionGroup("x", [])
except ValueError as e:
    print("empty exceptions:", type(e).__name__)

# ExceptionGroup validation: ExceptionGroup rejects non-Exception items
try:
    ExceptionGroup("x", [KeyboardInterrupt()])
except TypeError as e:
    print("ExceptionGroup rejects BaseException-only:", type(e).__name__)

# except* wraps a plain exception in an ExceptionGroup (PEP 654)
try:
    raise ValueError("plain")
except* ValueError as eg:
    print("plain exception wrapped:", type(eg).__name__, eg.exceptions)
