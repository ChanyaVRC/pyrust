"""Parity fixture for issue #1293.

Built-in functions raise TypeError (not RuntimeError) for wrong argument
counts and wrong argument types. This fixture covers the acceptance-criteria
cases from #1293 and verifies that except-TypeError clauses can catch these
errors.
"""


def capture(fn):
    try:
        fn()
        return "no error"
    except TypeError as e:
        return f"TypeError: {e}"
    except Exception as e:
        return f"{type(e).__name__}: {e}"


# next() — TypeError is catchable (the original repro from #1293)
caught = False
try:
    next()
except TypeError:
    caught = True
print(f"next() TypeError catchable: {caught}")

# issubclass wrong arity
print(capture(lambda: issubclass(int)))

# isinstance wrong arity
print(capture(lambda: isinstance(1)))

# hasattr wrong arity
print(capture(lambda: hasattr(object)))

# getattr — too few args
print(capture(lambda: getattr(object)))

# getattr — too many args
print(capture(lambda: getattr(object, "x", None, None)))

# abs wrong type
print(capture(lambda: abs("x")))

# hash — unhashable type
print(capture(lambda: hash([])))

# super — too many args
print(capture(lambda: super(int, int, int)))

# super — non-class first arg
print(capture(lambda: super(1, 2)))

# pow — too many args
print(capture(lambda: pow(2, 3, 4, 5)))

# classmethod() no args — TypeError
print(capture(lambda: classmethod()))

# staticmethod() no args — TypeError
print(capture(lambda: staticmethod()))

# classmethod(42) — must NOT raise; CPython 3.12 wraps any object
try:
    classmethod(42)
    print("classmethod(42): no error")
except TypeError as e:
    print(f"classmethod(42): TypeError: {e}")

# staticmethod(42) — must NOT raise
try:
    staticmethod(42)
    print("staticmethod(42): no error")
except TypeError as e:
    print(f"staticmethod(42): TypeError: {e}")

# len — wrong arity
print(capture(lambda: len([], [])))

# sorted — no args
print(capture(lambda: sorted()))

# sorted — too many positional args
print(capture(lambda: sorted([1, 2], [3, 4])))

# list — too many args
print(capture(lambda: list([], [])))

# tuple — too many args
print(capture(lambda: tuple((), ())))

# set — too many args
print(capture(lambda: set([], [])))

# frozenset — too many args
print(capture(lambda: frozenset([], [])))

# complex — too many args
print(capture(lambda: complex(1, 2, 3)))

# int — non-int base
print(capture(lambda: int("x", "y")))

# int — unsupported type
print(capture(lambda: int({})))

# float — unsupported type
print(capture(lambda: float({})))

# float — too many args
print(capture(lambda: float(1, 2)))
