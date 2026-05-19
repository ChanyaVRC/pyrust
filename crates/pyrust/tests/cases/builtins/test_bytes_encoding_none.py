# Parity fixture: bytes() encoding/errors argument type-error messages for None.
#
# CPython 3.12 reports "not None" (the singleton display name) rather than
# "not NoneType" (the type's __name__) when None is passed as the encoding or
# errors argument to bytes().  This differs from all other builtins, which use
# the type __name__ ("NoneType") in their error messages.
#
# See issue #795.

import sys

def capture(fn):
    try:
        fn()
        return "no error"
    except TypeError as e:
        return str(e)

# --- encoding argument ---

# None should display as "None" (not "NoneType")
msg = capture(lambda: bytes('hello', None))
assert 'not None' in msg and 'NoneType' not in msg, repr(msg)
print(msg)

# Other non-str types use the class name
msg = capture(lambda: bytes('hello', 42))
assert 'not int' in msg, repr(msg)
print(msg)

msg = capture(lambda: bytes('hello', True))
assert 'not bool' in msg, repr(msg)
print(msg)

msg = capture(lambda: bytes('hello', 1.5))
assert 'not float' in msg, repr(msg)
print(msg)

msg = capture(lambda: bytes('hello', []))
assert 'not list' in msg, repr(msg)
print(msg)

# --- errors argument ---

msg = capture(lambda: bytes('hello', 'utf-8', None))
assert 'not None' in msg and 'NoneType' not in msg, repr(msg)
print(msg)

msg = capture(lambda: bytes('hello', 'utf-8', 42))
assert 'not int' in msg, repr(msg)
print(msg)

# --- type(None).__name__ is still "NoneType" ---
# This ensures the fix only affects error message display, not the type system.
assert type(None).__name__ == 'NoneType'
print('type(None).__name__:', type(None).__name__)

# --- Other builtins still use "NoneType" for None ---
msg = capture(lambda: abs(None))
assert 'NoneType' in msg, repr(msg)
print(msg)

msg = capture(lambda: sorted(None))
assert 'NoneType' in msg, repr(msg)
print(msg)
