# Parity fixture for issue #1104: __doc__ on user-defined functions and classes.
# The compiler extracts the leading string literal from the body and exposes it
# as __doc__; functions/classes without a leading string literal have __doc__ == None.

# ── Functions ──────────────────────────────────────────────────────────────────

def greet(name):
    """Return a greeting for the given name."""
    return f'Hello, {name}'

def multiline():
    """First line.

    Second paragraph.
    """
    pass

def single():
    """Single-line triple-quoted."""
    pass

def plain():
    'Single-quoted docstring.'
    pass

def no_doc():
    return 42

def doc_after_stmt():
    x = 1
    "This is not a docstring."
    return x

def only_doc():
    "Just the doc."

print(repr(greet.__doc__))
print(repr(multiline.__doc__))
print(repr(single.__doc__))
print(repr(plain.__doc__))
print(no_doc.__doc__ is None)
print(doc_after_stmt.__doc__ is None)
print(repr(only_doc.__doc__))
print(only_doc())

# Implicit string concatenation is still a docstring
def concat_doc():
    "First part. " "Second part."

print(repr(concat_doc.__doc__))

# ── Classes ────────────────────────────────────────────────────────────────────

class Point:
    """A 2-D point."""
    def __init__(self, x, y):
        """Initialise point."""
        self.x, self.y = x, y

class NoDocClass:
    x = 0

class OnlyDoc:
    "Just the class doc."

print(repr(Point.__doc__))
print(repr(Point.__init__.__doc__))
print(NoDocClass.__doc__ is None)
print(repr(OnlyDoc.__doc__))

# __doc__ always appears in vars(C)
print('__doc__' in vars(Point))
print('__doc__' in vars(NoDocClass))

# ── Nested classes and functions ───────────────────────────────────────────────

class Outer:
    """Outer docstring."""
    class Inner:
        """Inner docstring."""
        pass

print(repr(Outer.__doc__))
print(repr(Outer.Inner.__doc__))

def make_fn():
    def inner():
        """Inner function doc."""
        pass
    return inner

print(repr(make_fn().__doc__))

# ── Mutability ─────────────────────────────────────────────────────────────────

def mutable():
    """Original."""
    pass

mutable.__doc__ = "Replaced."
print(repr(mutable.__doc__))

class CMutable:
    """Original class."""
    pass

CMutable.__doc__ = "Replaced class."
print(repr(CMutable.__doc__))

# ── Explicit __doc__ assignment in class body overrides docstring ──────────────

class Explicit:
    """Not this."""
    __doc__ = "Explicit override."

print(repr(Explicit.__doc__))

# Equality check matching CPython
print(greet.__doc__ == 'Return a greeting for the given name.')
