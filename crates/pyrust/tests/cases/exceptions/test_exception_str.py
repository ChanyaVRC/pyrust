# Issue #1151: user-defined __str__ on exception subclasses must be dispatched.
# CPython 3.12 calls the user __str__ when defined; only falls back to
# BaseException arg-formatting when no user __str__ exists.

class MyError(Exception):
    def __str__(self):
        return "custom error message"

e = MyError("something")
print(str(e))       # custom error message
print(f"{e!s}")     # custom error message

# No regression — without custom __str__, uses BaseException default.
e2 = Exception("test")
print(str(e2))      # test

# Exception with no args: empty string (not "()")
print(str(Exception()))   # (empty line)
