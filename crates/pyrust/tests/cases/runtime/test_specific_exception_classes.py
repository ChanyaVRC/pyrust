# Built-in operations raise the CPython-specific exception class, not
# the generic `RuntimeError` (#336).
#
# Each block triggers the operation, catches the *specific* class, and
# prints only the class name and message — by class.  If the class is
# wrong the `except` clause does not fire and the test diverges.

def check_class(name, expected, fn):
    try:
        fn()
    except Exception as e:
        actual = type(e).__name__
        if actual == expected:
            print(name, "->", actual, "OK")
        else:
            print(name, "->", actual, "EXPECTED", expected)
    else:
        print(name, "-> (no exception) EXPECTED", expected)

# KeyError on dict subscript / dict.pop without default
check_class("dict[missing]",    "KeyError", lambda: {}["x"])
check_class("dict.pop missing", "KeyError", lambda: {}.pop("x"))

# IndexError on list.pop from empty / out-of-range
check_class("[].pop()",         "IndexError", lambda: [].pop())
check_class("list[100]",        "IndexError", lambda: [1, 2, 3][100])

# TypeError on non-subscriptable / non-callable
x = 1
check_class("int subscript",    "TypeError", lambda: x[0])
check_class("int call",         "TypeError", lambda: x(2))

# TypeError on arity mismatch (class, not wording — wording differs)
def f0(): pass
check_class("arity too many",   "TypeError", lambda: f0(1))

# TypeError on unknown keyword argument
def f_a(a): pass
check_class("unknown kwarg",    "TypeError", lambda: f_a(b=1))

# NameError on undefined name
check_class("undef name",       "NameError", lambda: undef_name)

# ZeroDivisionError on int/0 / int%0 / float/0 / int//0
check_class("zero div int",     "ZeroDivisionError", lambda: 1 / 0)
check_class("zero mod int",     "ZeroDivisionError", lambda: 1 % 0)
check_class("zero div float",   "ZeroDivisionError", lambda: 1.0 / 0)
check_class("zero floor int",   "ZeroDivisionError", lambda: 1 // 0)

# AttributeError on missing attribute
check_class("getattr miss",     "AttributeError", lambda: x.nonexistent)

# TypeError on unhashable dict key
check_class("unhashable list",  "TypeError", lambda: {[1]: 2})


# Specific classes are catchable directly:
try:
    {}["y"]
except KeyError as e:
    print("explicit KeyError:", e)

try:
    1 / 0
except ZeroDivisionError as e:
    print("explicit ZeroDivisionError:", e)

try:
    undef_name2
except NameError as e:
    print("explicit NameError:", e)

# They're all `Exception` subclasses:
try:
    1 / 0
except Exception:
    print("Exception caught ZeroDivisionError")
