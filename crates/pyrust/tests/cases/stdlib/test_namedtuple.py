# collections.namedtuple — issue #1884.
#
# namedtuple returns a tuple subclass with named fields.  The parity
# harness asserts byte-identical output against CPython 3.12, covering
# field access, indexing, unpacking, _make / _asdict / _replace / _fields,
# the Typename(x=1, y=2) repr, equality with plain tuples, rename, and
# defaults.
#
# Reference: https://docs.python.org/3/library/collections.html#collections.namedtuple

from collections import namedtuple

Point = namedtuple('Point', ['x', 'y'])
p = Point(11, y=22)

# repr + field/index access
print(p)
print(p.x, p.y)
print(p[0], p[1])

# unpacking
a, b = p
print(a, b)

# tuple subclass relationships
print(isinstance(p, tuple))
print(p == (11, 22))
print(tuple(p))
print(p[0] + p[1])

# class-level introspection
print(Point._fields)
print(Point.__name__, type(p).__name__)
print(Point.__match_args__)

# _asdict / _make / _replace
print(p._asdict())
print(Point._make([3, 4]))
print(p._replace(x=100))

# field_names as a comma/space string
T = namedtuple('T', 'a, b c')
print(T._fields)

# single field needs a trailing comma internally; verify it works
S = namedtuple('S', 'only')
print(S(5), S(5)._fields)

# empty namedtuple
E = namedtuple('E', [])
print(E(), E._fields)

# defaults apply to the rightmost fields
Q = namedtuple('Q', 'a b c', defaults=[2, 3])
print(Q(1), Q(1, 20, 30))
print(Q._field_defaults)

# rename replaces invalid names with _N
R = namedtuple('R', ['a', 'def', 'b', 'a'], rename=True)
print(R._fields)

# explicit module
M = namedtuple('M', 'v', module='mymod')
print(M.__module__)


# error paths
def err(fn):
    try:
        fn()
    except (ValueError, TypeError) as e:
        print(type(e).__name__, str(e))


err(lambda: namedtuple('X', ['a', 'a']))
err(lambda: namedtuple('X', ['_a']))
err(lambda: namedtuple('X', ['class']))
err(lambda: namedtuple('X', ['1a']))
err(lambda: namedtuple('X', 'a b', defaults=[1, 2, 3]))
err(lambda: Point(1, 2)._replace(z=9))
