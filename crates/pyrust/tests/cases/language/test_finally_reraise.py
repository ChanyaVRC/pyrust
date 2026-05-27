# Regression tests for issue #1188: finally block must run when an exception
# is raised from inside an `except` handler body (bare re-raise or new raise).

# Case 1: bare re-raise inside except -> finally runs
def bare_reraise_with_finally():
    try:
        raise ValueError("v")
    except ValueError:
        raise
    finally:
        print("finally1")

try:
    bare_reraise_with_finally()
except ValueError:
    pass

# Case 2: new exception raised inside except -> finally runs
def new_raise_with_finally():
    try:
        raise ValueError("v")
    except ValueError:
        raise TypeError("t")
    finally:
        print("finally2")

try:
    new_raise_with_finally()
except TypeError:
    pass

# Case 3: finally still runs on normal try success path (no exception)
def normal_success_finally():
    try:
        x = 1
    except ValueError:
        pass
    finally:
        print("finally3")

normal_success_finally()

# Case 4: finally still runs when exception exits try body uncaught
def uncaught_exception_finally():
    try:
        raise ValueError("v")
    except TypeError:
        pass
    finally:
        print("finally4")

try:
    uncaught_exception_finally()
except ValueError:
    pass

# Case 5: bare re-raise with `except E as e:` binding
def reraise_with_as_var():
    try:
        raise ValueError("v")
    except ValueError as e:
        raise
    finally:
        print("finally5")

try:
    reraise_with_as_var()
except ValueError:
    pass

# Case 6: new raise with `except E as e:` binding (cause expression uses e)
def new_raise_with_as_var_cause():
    try:
        raise ValueError("v")
    except ValueError as e:
        raise TypeError("t") from e
    finally:
        print("finally6")

try:
    new_raise_with_as_var_cause()
except TypeError:
    pass

# Case 7: nested try/except/finally -- inner re-raise runs inner finally;
# outer finally also runs when inner exception propagates.
def nested_finally():
    try:
        try:
            raise ValueError("v")
        except ValueError:
            raise
        finally:
            print("finally7-inner")
    finally:
        print("finally7-outer")

try:
    nested_finally()
except ValueError:
    pass

# Case 8: multiple except handlers, second one re-raises; finally runs.
def multi_handler_reraise():
    try:
        raise ValueError("v")
    except TypeError:
        pass
    except ValueError:
        raise
    finally:
        print("finally8")

try:
    multi_raise_with_finally = multi_handler_reraise
    multi_raise_with_finally()
except ValueError:
    pass

# Case 9: bare except (no type) re-raises; finally runs.
def bare_except_reraise():
    try:
        raise ValueError("v")
    except:
        raise
    finally:
        print("finally9")

try:
    bare_except_reraise()
except ValueError:
    pass

# Case 10: raise X from Y where Y is a local (should not be clobbered by cleanup)
def raise_from_local():
    try:
        raise ValueError("v")
    except ValueError as e:
        raise RuntimeError("r") from e
    finally:
        print("finally10")

try:
    raise_from_local()
except RuntimeError as ex:
    # __cause__ should be the original ValueError
    print("cause:", ex.__cause__)

# Case 11: PEP 3110 -- `as` binding is unbound in the finally block after raise
def as_var_unbound_in_finally():
    try:
        raise ValueError("v")
    except ValueError as e:
        raise
    finally:
        try:
            _ = e  # should raise UnboundLocalError
            print("e is bound (wrong)")
        except UnboundLocalError:
            print("e is unbound (correct)")

try:
    as_var_unbound_in_finally()
except ValueError:
    pass

# Case 12: nested except handlers -- raise exits inner except body; both inner
# and outer finally blocks must run in innermost-first order.
def nested_except_both_finally():
    try:
        raise ValueError("v")
    except ValueError:
        try:
            raise TypeError("t")
        except TypeError:
            raise
        finally:
            print("finally12-inner")
    finally:
        print("finally12-outer")

try:
    nested_except_both_finally()
except TypeError:
    pass

# Case 13: triple-nested -- each level's finally runs in innermost-first order.
def triple_nested_finally():
    try:
        raise ValueError("v")
    except ValueError:
        try:
            raise TypeError("t")
        except TypeError:
            try:
                raise RuntimeError("r")
            except RuntimeError:
                raise
            finally:
                print("finally13-inner")
        finally:
            print("finally13-mid")
    finally:
        print("finally13-outer")

try:
    triple_nested_finally()
except RuntimeError:
    pass
