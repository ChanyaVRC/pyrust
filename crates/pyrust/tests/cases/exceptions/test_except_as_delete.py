# After `except E as var:` exits normally, CPython deletes `var` via DELETE_FAST.
# Accessing it afterwards should raise UnboundLocalError (not NameError),
# because the variable IS a local — it was assigned — but its slot was cleared.
# Issue #1277.

# Basic case: accessing deleted except-clause variable raises UnboundLocalError.
def basic():
    try:
        raise ValueError("x")
    except ValueError as e:
        pass
    try:
        _ = e
    except UnboundLocalError:
        return "UnboundLocalError"
    except NameError:
        return "NameError (wrong)"

print(basic())

# UnboundLocalError is a subclass of NameError — catching NameError also works.
def catch_name_error_superclass():
    try:
        raise ValueError("x")
    except ValueError as e:
        pass
    try:
        _ = e
    except NameError:
        return "caught via NameError"

print(catch_name_error_superclass())

# The variable is still accessible inside the handler body.
def accessible_inside():
    try:
        raise ValueError("hello")
    except ValueError as e:
        result = str(e)
    return result

print(accessible_inside())

# If the handler never ran (no exception), the variable was never assigned.
# Accessing it still raises UnboundLocalError (it is a local slot, just never written).
def no_exception_raised():
    try:
        pass
    except ValueError as e:
        pass
    try:
        _ = e
    except UnboundLocalError:
        return "UnboundLocalError (never assigned)"
    except NameError:
        return "NameError (wrong)"

print(no_exception_raised())

# Multiple handlers: each as-name is deleted after its handler exits.
def multi_handler():
    try:
        raise TypeError("t")
    except ValueError as e:
        pass
    except TypeError as f:
        pass
    try:
        _ = f
    except UnboundLocalError:
        return "UnboundLocalError for f"
    except NameError:
        return "NameError (wrong)"

print(multi_handler())

# Module scope: accessing a deleted except-clause variable raises NameError
# (not UnboundLocalError), because there is no local variable concept at module scope.
try:
    raise ValueError("mod")
except ValueError as _mod_e:
    pass
try:
    _ = _mod_e
except NameError:
    print("module scope NameError")
except UnboundLocalError:
    print("module scope UnboundLocalError (wrong)")
