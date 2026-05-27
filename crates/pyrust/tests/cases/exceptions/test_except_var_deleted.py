# After `except E as var:` exits, CPython deletes `var` via DELETE_FAST.
# Accessing it afterwards should raise UnboundLocalError (not NameError)
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

# Variable is accessible inside the except block body.
def accessible_inside():
    try:
        raise ValueError("hello")
    except ValueError as e:
        result = str(e)
    return result

print(accessible_inside())

# Re-assignment inside the handler: the variable is still deleted at block exit.
def reassign_inside():
    try:
        raise ValueError("x")
    except ValueError as e:
        e = "keep"
    try:
        _ = e
    except UnboundLocalError:
        return "UnboundLocalError"
    except NameError:
        return "NameError (wrong)"
    return "no error (wrong)"

print(reassign_inside())

# Nested except: each as-name is deleted independently after its own handler exits.
def nested_except():
    results = []
    try:
        raise ValueError("outer")
    except ValueError as e1:
        try:
            raise TypeError("inner")
        except TypeError as e2:
            results.append(str(e2))
        # e2 deleted here
        try:
            _ = e2
            results.append("e2: no error (wrong)")
        except UnboundLocalError:
            results.append("e2: UnboundLocalError")
    # e1 deleted here
    try:
        _ = e1
        results.append("e1: no error (wrong)")
    except UnboundLocalError:
        results.append("e1: UnboundLocalError")
    return results

for r in nested_except():
    print(r)

# The raised exception is UnboundLocalError, not NameError.
def check_type():
    try:
        raise ValueError("x")
    except ValueError as e:
        pass
    try:
        _ = e
    except Exception as err:
        return type(err).__name__

print(check_type())

# Module scope: accessing a deleted except-clause variable raises NameError
# (there is no local variable concept at module scope).
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
