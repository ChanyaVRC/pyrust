# PEP 3134 exception context attributes: __context__, __cause__, __suppress_context__
# These are available on every exception instance.  CPython stores them as C
# slots (not in __dict__), but they must be readable and return the right
# defaults.

# --- Fresh exception: all defaults ---
e = ValueError("test")
print(e.__context__)            # None
print(e.__cause__)              # None
print(e.__suppress_context__)   # False

# --- User-defined subclass: same defaults ---
class MyError(ValueError):
    pass

e2 = MyError("sub")
print(e2.__context__)           # None
print(e2.__cause__)             # None
print(e2.__suppress_context__)  # False

# --- raise X from Y sets __cause__ and __suppress_context__ = True ---
saved_inner = None
try:
    try:
        raise ValueError("inner")
    except ValueError as e:
        saved_inner = e
        raise RuntimeError("outer") from e
except RuntimeError as exc:
    print(exc.__cause__)            # inner
    print(exc.__context__)          # inner (also set by implicit chaining)
    print(exc.__suppress_context__) # True

# --- Implicit chaining: raise inside except sets __context__ ---
saved_inner2 = None
try:
    try:
        raise ValueError("inner2")
    except ValueError as e:
        saved_inner2 = e
        raise RuntimeError("outer2")
except RuntimeError as exc:
    print(exc.__cause__)            # None
    print(exc.__context__)          # inner2
    print(exc.__suppress_context__) # False

# --- raise X from None: __cause__=None, __suppress_context__=True, __context__ still set ---
try:
    try:
        raise ValueError("inner3")
    except ValueError:
        raise RuntimeError("outer3") from None
except RuntimeError as exc:
    print(exc.__cause__)            # None
    print(exc.__context__)          # inner3
    print(exc.__suppress_context__) # True

# --- setattr: these attrs can be set directly ---
e3 = TypeError("writable")
e3.__suppress_context__ = True
print(e3.__suppress_context__)  # True
e3.__suppress_context__ = False
print(e3.__suppress_context__)  # False

ctx_val = ValueError("ctx")
e3.__context__ = ctx_val
print(e3.__context__ is ctx_val)  # True
e3.__context__ = None
print(e3.__context__)             # None

cause_val = OSError("cause")
e3.__cause__ = cause_val
print(e3.__cause__ is cause_val)  # True
e3.__cause__ = None
print(e3.__cause__)               # None

# --- Type validation: __cause__/__context__ must be None or BaseException; __suppress_context__ must be bool ---
e5 = ValueError("typecheck")
try:
    e5.__cause__ = "not an exception"
except TypeError as te:
    print(type(te).__name__)  # TypeError

try:
    e5.__context__ = 42
except TypeError as te:
    print(type(te).__name__)  # TypeError

try:
    e5.__suppress_context__ = "not a bool"
except TypeError as te:
    print(type(te).__name__)  # TypeError

# --- StopIteration: has .value and also the context attrs ---
e4 = StopIteration(42)
print(e4.value)                  # 42
print(e4.__context__)            # None
print(e4.__cause__)              # None
print(e4.__suppress_context__)   # False
