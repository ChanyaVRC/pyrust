# isinstance / issubclass — tuple-of-types parity (#383)
#
# CPython accepts a tuple as the second argument to isinstance() and
# issubclass(); the predicate is true if *any* element of the tuple
# matches.  Nested tuples are recursed into.

# --- basic happy path -----------------------------------------------------
assert isinstance(5, (int, str)) is True
assert isinstance("hi", (int, str)) is True
assert isinstance(5.0, (int, float)) is True
assert isinstance([], (list, tuple)) is True
assert isinstance((), (list, tuple)) is True

# --- non-match returns False, not error -----------------------------------
assert isinstance([], (int, str)) is False
assert isinstance(5, (str, list)) is False
assert isinstance(None, (int, str)) is False

# --- nested tuples are flattened recursively (CPython contract) -----------
assert isinstance(5, (int, (str, list))) is True
assert isinstance("x", (int, (str, list))) is True
assert isinstance([], (int, (str, list))) is True
assert isinstance(3.14, (int, (str, list))) is False
assert isinstance(5, ((int,), (str,))) is True

# --- empty tuple is always False ------------------------------------------
assert isinstance(5, ()) is False

# --- single-type form still works -----------------------------------------
assert isinstance(5, int) is True
assert isinstance("x", str) is True
assert isinstance(5, str) is False

# --- user-defined classes through tuple -----------------------------------
class A:
    pass

class B(A):
    pass

class C:
    pass

assert isinstance(B(), (A, C)) is True
assert isinstance(C(), (A, B)) is False
assert isinstance(B(), (C, (A,))) is True

# --- issubclass: tuple form -----------------------------------------------
assert issubclass(bool, (int, str)) is True
assert issubclass(int, (str, list)) is False
assert issubclass(B, (C, A)) is True
assert issubclass(C, (A, B)) is False

# --- issubclass: nested tuples --------------------------------------------
assert issubclass(B, (C, (A,))) is True
assert issubclass(bool, ((int,),)) is True

# --- TypeError parity on a non-class non-tuple second arg -----------------
try:
    isinstance(5, 42)
except TypeError:
    pass
else:
    raise AssertionError("isinstance(5, 42) should raise TypeError")

try:
    issubclass(int, 42)
except TypeError:
    pass
else:
    raise AssertionError("issubclass(int, 42) should raise TypeError")

print("ok")
