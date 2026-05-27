"""Parity fixture: int(float) and math.floor/ceil/trunc on non-finite floats.

CPython 3.12 raises OverflowError for infinity and ValueError for NaN when
converting a float to int via int(), math.floor(), math.ceil(), math.trunc().
Normal finite floats (including very large ones) must still truncate correctly.
"""
import math


def check(label, fn):
    try:
        result = fn()
        print(label + " = " + repr(result))
    except Exception as e:
        print(label + " => " + type(e).__name__ + ": " + str(e))


# --- int() built-in ---
check("int(1.5)", lambda: int(1.5))
check("int(-2.9)", lambda: int(-2.9))
check("int(0.0)", lambda: int(0.0))
check("int(-0.0)", lambda: int(-0.0))
check("int(float('inf'))", lambda: int(float("inf")))
check("int(float('-inf'))", lambda: int(float("-inf")))
check("int(float('nan'))", lambda: int(float("nan")))
# large finite float must still produce the correct big integer
check("type(int(1e300)).__name__", lambda: type(int(1e300)).__name__)

# --- math.floor ---
check("math.floor(2.9)", lambda: math.floor(2.9))
check("math.floor(-2.1)", lambda: math.floor(-2.1))
check("math.floor(float('inf'))", lambda: math.floor(float("inf")))
check("math.floor(float('-inf'))", lambda: math.floor(float("-inf")))
check("math.floor(float('nan'))", lambda: math.floor(float("nan")))

# --- math.ceil ---
check("math.ceil(2.1)", lambda: math.ceil(2.1))
check("math.ceil(-2.9)", lambda: math.ceil(-2.9))
check("math.ceil(float('inf'))", lambda: math.ceil(float("inf")))
check("math.ceil(float('-inf'))", lambda: math.ceil(float("-inf")))
check("math.ceil(float('nan'))", lambda: math.ceil(float("nan")))

# --- math.trunc ---
check("math.trunc(2.9)", lambda: math.trunc(2.9))
check("math.trunc(-2.9)", lambda: math.trunc(-2.9))
check("math.trunc(float('inf'))", lambda: math.trunc(float("inf")))
check("math.trunc(float('-inf'))", lambda: math.trunc(float("-inf")))
check("math.trunc(float('nan'))", lambda: math.trunc(float("nan")))
