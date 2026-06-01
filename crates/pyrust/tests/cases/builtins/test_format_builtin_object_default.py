# Parity fixture for issue #1924:
# Built-in types that inherit the default object.__format__ (NoneType, list,
# tuple, dict, set, frozenset, bytes, bytearray, range, type, function,
# builtin_function_or_method, ...) raise TypeError for any non-empty format
# spec, mirroring CPython 3.12. Empty spec returns str(value) unchanged, and
# types with a real __format__ (str/int/bool/float/complex) are unaffected.


def my_func():
    pass


# Non-empty spec on default-object.__format__ types → TypeError with the exact
# "<type>.__format__" wording (type name must match CPython precisely).
cases = [
    ("None", None, "5"),
    ("list", [1, 2], ">10"),
    ("tuple", (1,), "^8"),
    ("dict", {1: 2}, ">5"),
    ("set", {1, 2}, ">5"),
    ("frozenset", frozenset([1]), ">5"),
    ("bytes", b"ab", ">5"),
    ("bytearray", bytearray(b"ab"), ">8"),
    ("range", range(3), ">8"),
    ("type", int, ">8"),
    ("builtin_fn", len, ">5"),
    ("user_fn", my_func, ">5"),
]
for name, value, spec in cases:
    try:
        format(value, spec)
        print(name, "no error")
    except TypeError as e:
        print(name, "TypeError:", e)
    except ValueError as e:
        print(name, "ValueError:", e)

# f-string path raises too.
try:
    print(f"{None:5}")
except TypeError as e:
    print("fstring TypeError:", e)

# Empty spec returns str(value) unchanged for these types.
print(repr(format(None, "")))
print(repr(format([1, 2], "")))
print(repr(format((1,), "")))
print(repr(format({1: 2}, "")))
print(repr(format(b"ab", "")))
print(repr(format(len, "")) != "")

# Types with a real __format__ are unaffected.
print(format(True, "d"))
print(format(5, "x"))
print(format(3.14, ".2f"))
print(format("hi", ">5"))
print(format(1 + 2j, ">10"))
