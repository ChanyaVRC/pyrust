"""
Operator Interface — pyrust port of CPython 3.12's pure-Python
``Lib/operator.py``.

Every function corresponds to an intrinsic Python operator (``operator.add(x,
y) == x + y``), plus the generalized lookup helpers ``itemgetter`` /
``attrgetter`` / ``methodcaller``.  The bodies are deliberately kept close to
CPython's reference implementation so behaviour — including error messages and
edge cases — matches byte-for-byte.

This source is exec'd once into a throwaway namespace at first ``import
operator`` and the public names are copied onto the module by
``operator.rs::inject_python_members`` (mirrors the ``collections`` / ``asyncio``
injection, issue #2514).
"""

from builtins import abs as _abs

__all__ = [
    'abs', 'add', 'and_', 'attrgetter', 'call', 'concat', 'contains',
    'countOf', 'delitem', 'eq', 'floordiv', 'ge', 'getitem', 'gt', 'iadd',
    'iand', 'iconcat', 'ifloordiv', 'ilshift', 'imatmul', 'imod', 'imul',
    'index', 'indexOf', 'inv', 'invert', 'ior', 'ipow', 'irshift', 'is_',
    'is_not', 'isub', 'itemgetter', 'itruediv', 'ixor', 'le', 'length_hint',
    'lshift', 'lt', 'matmul', 'methodcaller', 'mod', 'mul', 'ne', 'neg',
    'not_', 'or_', 'pos', 'pow', 'rshift', 'setitem', 'sub', 'truediv',
    'truth', 'xor'
]

# Comparison Operations *******************************************************#

def lt(a, b):
    "Same as a < b."
    return a < b

def le(a, b):
    "Same as a <= b."
    return a <= b

def eq(a, b):
    "Same as a == b."
    return a == b

def ne(a, b):
    "Same as a != b."
    return a != b

def ge(a, b):
    "Same as a >= b."
    return a >= b

def gt(a, b):
    "Same as a > b."
    return a > b

# Logical Operations **********************************************************#

def not_(a):
    "Same as not a."
    return not a

def truth(a):
    "Return True if a is true, False otherwise."
    return True if a else False

def is_(a, b):
    "Same as a is b."
    return a is b

def is_not(a, b):
    "Same as a is not b."
    return a is not b

# Mathematical/Bitwise Operations *********************************************#

def abs(a):
    "Same as abs(a)."
    return _abs(a)

def add(a, b):
    "Same as a + b."
    return a + b

def and_(a, b):
    "Same as a & b."
    return a & b

def floordiv(a, b):
    "Same as a // b."
    return a // b

def index(a):
    "Same as a.__index__()."
    # This pure-Python reference is intentionally not exported: CPython's
    # public operator.index is the accelerated PyNumber_Index wrapper, which
    # pyrust declares natively beside length_hint.
    try:
        m = type(a).__index__
    except AttributeError:
        raise TypeError("'%s' object cannot be interpreted as an integer"
                        % type(a).__name__)
    return m(a)

def inv(a):
    "Same as ~a."
    return ~a
invert = inv

def lshift(a, b):
    "Same as a << b."
    return a << b

def mod(a, b):
    "Same as a % b."
    return a % b

def mul(a, b):
    "Same as a * b."
    return a * b

def matmul(a, b):
    "Same as a @ b."
    return a @ b

def neg(a):
    "Same as -a."
    return -a

def or_(a, b):
    "Same as a | b."
    return a | b

def pos(a):
    "Same as +a."
    return +a

def pow(a, b):
    "Same as a ** b."
    return a ** b

def rshift(a, b):
    "Same as a >> b."
    return a >> b

def sub(a, b):
    "Same as a - b."
    return a - b

def truediv(a, b):
    "Same as a / b."
    return a / b

def xor(a, b):
    "Same as a ^ b."
    return a ^ b

# Sequence Operations *********************************************************#

def concat(a, b):
    "Same as a + b, for a and b sequences."
    if not hasattr(a, '__getitem__'):
        msg = "'%s' object can't be concatenated" % type(a).__name__
        raise TypeError(msg)
    return a + b

def contains(a, b):
    "Same as b in a (note reversed operands)."
    return b in a

def countOf(a, b):
    "Return the number of items in a which are, or which equal, b."
    count = 0
    for i in a:
        if i is b or i == b:
            count += 1
    return count

def delitem(a, b):
    "Same as del a[b]."
    del a[b]

def getitem(a, b):
    "Same as a[b]."
    return a[b]

def indexOf(a, b):
    "Return the first index of b in a."
    for i, j in enumerate(a):
        if j is b or j == b:
            return i
    else:
        raise ValueError('sequence.index(x): x not in sequence')

def setitem(a, b, c):
    "Same as a[b] = c."
    a[b] = c

# `length_hint` is NOT defined here.  CPython's ``Lib/operator.py`` ends with
# ``from _operator import *``, so the accelerated C definition wins at runtime,
# and for ``length_hint`` the two implementations observably differ (the C one
# coerces ``default`` through ``__index__``, narrows to ``Py_ssize_t``, rejects
# keyword arguments, and normalises a ``bool`` result to ``1``).  pyrust
# therefore declares it natively in ``operator.rs`` so it can reach the
# interpreter's shared ``PyObject_LengthHint`` protocol — the same protocol the
# built-in iterators answer from (issue #2920).

# Other Operations ************************************************************#

def call(obj, /, *args, **kwargs):
    """Same as obj(*args, **kwargs)."""
    return obj(*args, **kwargs)

# Generalized Lookup Objects **************************************************#

class attrgetter:
    """
    Return a callable object that fetches the given attribute(s) from its operand.
    After f = attrgetter('name'), the call f(r) returns r.name.
    After g = attrgetter('name', 'date'), the call g(r) returns (r.name, r.date).
    After h = attrgetter('name.first', 'name.last'), the call h(r) returns
    (r.name.first, r.name.last).
    """
    __slots__ = ('_attrs', '_call')

    def __init__(self, *attrs):
        # CPython's C constructor reports `expected 1 argument, got 0` for the
        # no-argument case (issue #2514); the pure-Python reference would
        # instead surface a `missing required positional argument` message, so
        # we take all attrs variadically and validate explicitly.
        if not attrs:
            raise TypeError('attrgetter expected 1 argument, got 0')
        if len(attrs) == 1:
            attr = attrs[0]
            if not isinstance(attr, str):
                raise TypeError('attribute name must be a string')
            self._attrs = (attr,)
            names = attr.split('.')
            def func(obj):
                for name in names:
                    obj = getattr(obj, name)
                return obj
            self._call = func
        else:
            self._attrs = attrs
            getters = tuple(map(attrgetter, self._attrs))
            def func(obj):
                return tuple(getter(obj) for getter in getters)
            self._call = func

    def __call__(self, obj):
        return self._call(obj)

    def __repr__(self):
        return '%s.%s(%s)' % (self.__class__.__module__,
                              self.__class__.__qualname__,
                              ', '.join(map(repr, self._attrs)))

    def __reduce__(self):
        return self.__class__, self._attrs

class itemgetter:
    """
    Return a callable object that fetches the given item(s) from its operand.
    After f = itemgetter(2), the call f(r) returns r[2].
    After g = itemgetter(2, 5, 3), the call g(r) returns (r[2], r[5], r[3])
    """
    __slots__ = ('_items', '_call')

    def __init__(self, *items):
        # See `attrgetter.__init__` — match the C constructor's no-argument
        # message rather than the pure-Python reference's (issue #2514).
        if not items:
            raise TypeError('itemgetter expected 1 argument, got 0')
        if len(items) == 1:
            item = items[0]
            self._items = (item,)
            def func(obj):
                return obj[item]
            self._call = func
        else:
            self._items = items
            def func(obj):
                return tuple(obj[i] for i in items)
            self._call = func

    def __call__(self, obj):
        return self._call(obj)

    def __repr__(self):
        return '%s.%s(%s)' % (self.__class__.__module__,
                              self.__class__.__name__,
                              ', '.join(map(repr, self._items)))

    def __reduce__(self):
        return self.__class__, self._items

class methodcaller:
    """
    Return a callable object that calls the given method on its operand.
    After f = methodcaller('name'), the call f(r) returns r.name().
    After g = methodcaller('name', 'date', foo=1), the call g(r) returns
    r.name('date', foo=1).
    """
    __slots__ = ('_name', '_args', '_kwargs')

    def __init__(self, *args, **kwargs):
        # The C constructor reports a dedicated message when the method name is
        # omitted (issue #2514); take `name` variadically so we can emit it.
        if not args:
            raise TypeError(
                'methodcaller needs at least one argument, the method name')
        self._name = args[0]
        if not isinstance(self._name, str):
            raise TypeError('method name must be a string')
        self._args = args[1:]
        self._kwargs = kwargs

    def __call__(self, obj):
        return getattr(obj, self._name)(*self._args, **self._kwargs)

    def __repr__(self):
        args = [repr(self._name)]
        args += [repr(a) for a in self._args]
        args += ['%s=%r' % (k, v) for k, v in self._kwargs.items()]
        return '%s.%s(%s)' % (self.__class__.__module__,
                              self.__class__.__name__,
                              ', '.join(args))

    def __reduce__(self):
        if not self._kwargs:
            return self.__class__, (self._name,) + self._args
        else:
            from functools import partial
            return partial(self.__class__, self._name, **self._kwargs), self._args


# In-place Operations *********************************************************#

def iadd(a, b):
    "Same as a += b."
    a += b
    return a

def iand(a, b):
    "Same as a &= b."
    a &= b
    return a

def iconcat(a, b):
    "Same as a += b, for a and b sequences."
    if not hasattr(a, '__getitem__'):
        msg = "'%s' object can't be concatenated" % type(a).__name__
        raise TypeError(msg)
    a += b
    return a

def ifloordiv(a, b):
    "Same as a //= b."
    a //= b
    return a

def ilshift(a, b):
    "Same as a <<= b."
    a <<= b
    return a

def imod(a, b):
    "Same as a %= b."
    a %= b
    return a

def imul(a, b):
    "Same as a *= b."
    a *= b
    return a

def imatmul(a, b):
    "Same as a @= b."
    a @= b
    return a

def ior(a, b):
    "Same as a |= b."
    a |= b
    return a

def ipow(a, b):
    "Same as a **= b."
    a **= b
    return a

def irshift(a, b):
    "Same as a >>= b."
    a >>= b
    return a

def isub(a, b):
    "Same as a -= b."
    a -= b
    return a

def itruediv(a, b):
    "Same as a /= b."
    a /= b
    return a

def ixor(a, b):
    "Same as a ^= b."
    a ^= b
    return a
