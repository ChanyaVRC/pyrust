# Parity fixture for error cases in `from module import *` (issue #926).
#
# These helpers are in _all_with_*.py (no test_ prefix, so the harness
# ignores them as standalone scripts but imports work because they sit in
# the same directory as this file).

# ── Missing name in __all__ must raise AttributeError ─────────────────────────

try:
    from _all_with_missing import *
    print("ERROR: expected AttributeError not raised")
except AttributeError as e:
    print("AttributeError:", e)

# ── Non-string item in __all__ must raise TypeError ───────────────────────────

try:
    from _all_with_bad_type import *
    print("ERROR: expected TypeError not raised")
except TypeError as e:
    print("TypeError:", e)
