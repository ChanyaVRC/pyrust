"""
Parity fixture for nested try/except and sys.exception() / sys.exc_info().

CPython 3.12 rule: entering an except block sets sys.exception() to the
caught exception; leaving the block (EndExcept) restores it to whatever
it was when the except block was entered.  This must hold across arbitrary
nesting levels.
"""

import sys

# ── 1. Baseline: no active exception outside any handler ─────────────────────
print(sys.exception() is None)   # True

# ── 2. Single-level: exception is visible inside the handler ─────────────────
try:
    raise ValueError("single")
except ValueError:
    print(type(sys.exception()).__name__)   # ValueError

# After handler exits, active exception is gone
print(sys.exception() is None)   # True

# ── 3. Two-level nesting: outer exception restored after inner handler exits ──
try:
    raise TypeError("outer")
except TypeError:
    print(type(sys.exception()).__name__)   # TypeError — outer visible

    try:
        raise ValueError("inner")
    except ValueError:
        print(type(sys.exception()).__name__)   # ValueError — inner visible

    # Inner handler has exited; outer handler's exception must be restored
    print(type(sys.exception()).__name__)   # TypeError — must be restored

print(sys.exception() is None)   # True — back to nothing

# ── 4. Three-level nesting ────────────────────────────────────────────────────
try:
    raise TypeError("L1")
except TypeError:
    try:
        raise ValueError("L2")
    except ValueError:
        try:
            raise KeyError("L3")
        except KeyError:
            print(type(sys.exception()).__name__)   # KeyError

        # After L3 handler exits, L2 exception restored
        print(type(sys.exception()).__name__)   # ValueError

    # After L2 handler exits, L1 exception restored
    print(type(sys.exception()).__name__)   # TypeError

print(sys.exception() is None)   # True

# ── 5. sys.exc_info() parity ─────────────────────────────────────────────────
try:
    raise RuntimeError("exc_info_test")
except RuntimeError:
    tp, val, _ = sys.exc_info()
    print(tp.__name__)          # RuntimeError
    print(str(val))             # exc_info_test

    try:
        raise StopIteration("nested")
    except StopIteration:
        tp2, val2, _ = sys.exc_info()
        print(tp2.__name__)     # StopIteration
        print(str(val2))        # nested

    # Restored to outer
    tp3, val3, _ = sys.exc_info()
    print(tp3.__name__)         # RuntimeError
    print(str(val3))            # exc_info_test

# ── 6. Handlers that contain no nested try/except are unaffected ──────────────
try:
    raise LookupError("no nesting")
except LookupError:
    print(type(sys.exception()).__name__)   # LookupError

print(sys.exception() is None)   # True

# ── 7. Re-raise inside nested except: outer handler still sees its exception ──
try:
    raise TypeError("outer_rerase")
except TypeError:
    try:
        try:
            raise ValueError("to_reraise")
        except ValueError:
            raise   # bare re-raise
    except ValueError:
        pass
    print(type(sys.exception()).__name__)   # TypeError — outer still active

print(sys.exception() is None)   # True
