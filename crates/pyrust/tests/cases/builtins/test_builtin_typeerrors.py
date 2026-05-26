"""Builtin functions raise TypeError (not RuntimeError) for wrong arg counts/types.

Issue #1293: builtins.rs used PyError::Runtime where PyError::named("TypeError", …)
is required, causing except TypeError: to silently fail.
"""


def capture(fn):
    try:
        fn()
        return "no error"
    except TypeError as e:
        return f"TypeError: {e}"
    except Exception as e:
        return f"{type(e).__name__}: {e}"


# next() — wrong arity
print(capture(lambda: next()))

# next() is catchable as TypeError
caught = False
try:
    next()
except TypeError:
    caught = True
print(f"next() TypeError catchable: {caught}")

# issubclass() — wrong arity
print(capture(lambda: issubclass(int)))

# isinstance() — wrong arity
print(capture(lambda: isinstance(1)))

# type() — wrong arity (2 args: not 1 and not 3)
print(capture(lambda: type(1, 2)))

# hasattr() — wrong arity
print(capture(lambda: hasattr(object)))

# getattr() — too few args
print(capture(lambda: getattr(object)))

# getattr() — too many args
print(capture(lambda: getattr(object, "x", None, None)))

# len() — wrong arity
print(capture(lambda: len(1, 2)))

# sorted() — no args
print(capture(lambda: sorted()))

# sorted() — too many positional args
print(capture(lambda: sorted([1, 2], [3, 4])))

# list() — too many args
print(capture(lambda: list([], [])))

# tuple() — too many args
print(capture(lambda: tuple((), ())))

# complex() — too many args
print(capture(lambda: complex(1, 2, 3)))

# set() — too many args
print(capture(lambda: set([], [])))

# frozenset() — too many args
print(capture(lambda: frozenset([], [])))

# classmethod() — no args
print(capture(lambda: classmethod()))

# staticmethod() — no args
print(capture(lambda: staticmethod()))

# property() — too many args
print(capture(lambda: property(None, None, None, None, None)))

# property() — bad keyword
print(capture(lambda: property(fget=None, bad_key=None)))

# super() — too many args
print(capture(lambda: super(int, int, int)))

# pow() — no args
print(capture(lambda: pow()))

# pow() — only one arg
print(capture(lambda: pow(2)))

# pow() — too many args
print(capture(lambda: pow(2, 3, 4, 5)))
