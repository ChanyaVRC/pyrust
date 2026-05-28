# Test __context__ chain when finally raises after a new raise in an except handler.
# Issue #1396: when raise X exits an except body and the inlined finally also raises,
# the new exception's __context__ should be X (not the originally-caught exception).

# Primary case: raise X in except, finally raises — context should be X
def f_primary():
    try:
        raise ValueError("v")
    except ValueError:
        raise TypeError("t")
    finally:
        raise RuntimeError("f")

try:
    f_primary()
except RuntimeError as ex:
    print("primary context:", type(ex.__context__).__name__)

# Bare re-raise in except, finally raises — context should be the originally-caught exc
def f_bare_reraise():
    try:
        raise ValueError("v")
    except ValueError:
        raise
    finally:
        raise RuntimeError("f")

try:
    f_bare_reraise()
except RuntimeError as ex:
    print("bare reraise context:", type(ex.__context__).__name__)

# No finally, raise X in except — context is the caught exception
def f_no_finally():
    try:
        raise ValueError("v")
    except ValueError:
        raise TypeError("t")

try:
    f_no_finally()
except TypeError as ex:
    print("no finally context:", type(ex.__context__).__name__)

# Finally completes normally — TypeError propagates with ValueError context
def f_finally_normal():
    try:
        raise ValueError("v")
    except ValueError:
        raise TypeError("t")
    finally:
        pass

try:
    f_finally_normal()
except TypeError as ex:
    print("finally normal context:", type(ex.__context__).__name__)

# Outer finally: raise X in inner except, outer finally raises
def f_outer_finally():
    try:
        try:
            raise ValueError("v")
        except ValueError:
            raise TypeError("t")
    finally:
        raise RuntimeError("f")

try:
    f_outer_finally()
except RuntimeError as ex:
    print("outer finally context:", type(ex.__context__).__name__)

# finally raises, except consumed the exception — context is None
def f_consumed():
    try:
        raise ValueError("v")
    except ValueError:
        pass
    finally:
        raise RuntimeError("f")

try:
    f_consumed()
except RuntimeError as ex:
    ctx = ex.__context__
    print("consumed context:", type(ctx).__name__ if ctx else None)

# raise X from cause in except, finally raises — context is X
def f_raise_from():
    cause = KeyError("k")
    try:
        raise ValueError("v")
    except ValueError:
        raise TypeError("t") from cause
    finally:
        raise RuntimeError("f")

try:
    f_raise_from()
except RuntimeError as ex:
    print("raise from context:", type(ex.__context__).__name__)

# Finally catches its own exception internally — TypeError context must still be ValueError.
# Regression: PushExcContext entry was incorrectly removed by handle_vm_error's
# duplicate-detection when KeyError was dispatched to the inner except handler.
def f_finally_catches_internally():
    try:
        raise ValueError("v")
    except ValueError:
        raise TypeError("t")
    finally:
        try:
            raise KeyError("k")
        except KeyError:
            pass

try:
    f_finally_catches_internally()
except TypeError as ex:
    print("finally catches internally context:", type(ex.__context__).__name__)
