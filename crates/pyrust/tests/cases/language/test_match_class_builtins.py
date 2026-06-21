"""
Self-capturing class patterns for built-in atomic types (PEP 634 §3.4).

For bool, bytearray, bytes, dict, float, frozenset, int, list, set, str and
tuple a single positional sub-pattern captures the whole subject instead of
consulting __match_args__. (Note: CPython 3.12 deliberately excludes `complex`
from this list even though PEP 634's prose mentions it.)
"""

# --- Single positional sub-pattern captures the subject ---

match 5:
    case int(n):
        print("int", n)
# int 5

match "hello":
    case str(s):
        print("str", s)
# str hello

match 3.14:
    case float(f):
        print("float", f)
# float 3.14

match b"hi":
    case bytes(b):
        print("bytes", b)
# bytes b'hi'

match bytearray(b"xy"):
    case bytearray(ba):
        print("bytearray", ba)
# bytearray bytearray(b'xy')

match True:
    case bool(x):
        print("bool", x)
# bool True

match [1, 2]:
    case list(l):
        print("list", l)
# list [1, 2]

match {"a": 1}:
    case dict(d):
        print("dict", d)
# dict {'a': 1}

match {1, 2}:
    case set(st):
        print("set", sorted(st))
# set [1, 2]

match frozenset({3}):
    case frozenset(fs):
        print("frozenset", sorted(fs))
# frozenset [3]

match (1, 2):
    case tuple(t):
        print("tuple", t)
# tuple (1, 2)

# --- isinstance check still happens first ---

match "abc":
    case int(n):
        print("int", n)
    case str(s):
        print("nonmatch->str", s)
# nonmatch->str abc

# --- More than one positional sub-pattern is a TypeError ---

try:
    match 5:
        case int(a, b):
            print("matched", a, b)
except TypeError as e:
    print("TypeError:", e)
# TypeError: int() accepts 1 positional sub-pattern (2 given)

# --- Keyword sub-patterns still read attributes ---

match 5:
    case int(real=r):
        print("real", r)
# real 5

# --- A user class named "int" does NOT self-capture ---

_real_int = int


class int:  # noqa: A001 - intentionally shadows the builtin
    __match_args__ = ()


try:
    match int():
        case int(z):
            print("user int matched", z)
        case _:
            print("user int: no positional")
except TypeError as e:
    print("TypeError:", e)
# TypeError: int() accepts 0 positional sub-patterns (1 given)
