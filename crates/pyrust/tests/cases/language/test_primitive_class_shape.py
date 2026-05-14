# Issue #462: primitive built-in types are proper `class` values.
#
# `int`, `str`, `list`, `tuple`, `dict`, `set`, `frozenset`, `bytes`,
# `complex`, `float`, and `bool` now carry the `PyClass` shape — they
# answer `__name__`, `__bases__`, `__mro__`, support `isinstance` via
# the standard class-hierarchy walk, and `type(value) is T` holds.
#
# `bool` inherits from `int` to match CPython's `bool.__bases__ == (int,)`.
# The other ten chain directly to the synthetic `object` class.

# ── __name__ ──────────────────────────────────────────────────────────
print(int.__name__)        # int
print(str.__name__)        # str
print(float.__name__)      # float
print(bool.__name__)       # bool
print(list.__name__)       # list
print(tuple.__name__)      # tuple
print(dict.__name__)       # dict
print(set.__name__)        # set
print(frozenset.__name__)  # frozenset
print(bytes.__name__)      # bytes
print(complex.__name__)    # complex

# ── __bases__ ─────────────────────────────────────────────────────────
print(int.__bases__)        # (<class 'object'>,)
print(str.__bases__)        # (<class 'object'>,)
print(float.__bases__)      # (<class 'object'>,)
print(bool.__bases__)       # (<class 'int'>,)
print(list.__bases__)       # (<class 'object'>,)
print(tuple.__bases__)      # (<class 'object'>,)
print(dict.__bases__)       # (<class 'object'>,)
print(set.__bases__)        # (<class 'object'>,)
print(frozenset.__bases__)  # (<class 'object'>,)
print(bytes.__bases__)      # (<class 'object'>,)
print(complex.__bases__)    # (<class 'object'>,)

# ── __mro__ ───────────────────────────────────────────────────────────
print([c.__name__ for c in int.__mro__])        # ['int', 'object']
print([c.__name__ for c in str.__mro__])        # ['str', 'object']
print([c.__name__ for c in float.__mro__])      # ['float', 'object']
print([c.__name__ for c in bool.__mro__])       # ['bool', 'int', 'object']
print([c.__name__ for c in list.__mro__])       # ['list', 'object']
print([c.__name__ for c in tuple.__mro__])      # ['tuple', 'object']
print([c.__name__ for c in dict.__mro__])       # ['dict', 'object']
print([c.__name__ for c in set.__mro__])        # ['set', 'object']
print([c.__name__ for c in frozenset.__mro__])  # ['frozenset', 'object']
print([c.__name__ for c in bytes.__mro__])      # ['bytes', 'object']
print([c.__name__ for c in complex.__mro__])    # ['complex', 'object']

# ── type(value) is T ──────────────────────────────────────────────────
print(type(0) is int)              # True
print(type("") is str)             # True
print(type(0.0) is float)          # True
print(type(True) is bool)          # True
print(type([]) is list)            # True
print(type(()) is tuple)           # True
print(type({}) is dict)            # True
print(type(set()) is set)          # True
print(type(frozenset()) is frozenset)  # True
print(type(b"") is bytes)          # True
print(type(0j) is complex)         # True

# Asymmetric — `type(True)` is exactly `bool`, not `int`, but `bool`
# values still report as int via isinstance.
print(type(True) is int)           # False
print(isinstance(True, int))       # True
print(isinstance(True, bool))      # True
print(isinstance(1, bool))         # False

# ── isinstance through the class hierarchy ────────────────────────────
print(isinstance(0, int))          # True
print(isinstance("", str))         # True
print(isinstance(0.0, float))      # True
print(isinstance(True, bool))      # True
print(isinstance([], list))        # True
print(isinstance((), tuple))       # True
print(isinstance({}, dict))        # True
print(isinstance(set(), set))      # True
print(isinstance(frozenset(), frozenset))  # True
print(isinstance(b"", bytes))      # True
print(isinstance(0j, complex))     # True

# Tuple-of-classes form, mixing migrated primitives.
print(isinstance(0, (int, str)))      # True
print(isinstance("a", (int, str)))    # True
print(isinstance(0.0, (int, str)))    # False
print(isinstance(0.0, (int, float)))  # True

# ── issubclass ────────────────────────────────────────────────────────
print(issubclass(bool, int))        # True (bool inherits from int)
print(issubclass(int, bool))        # False
print(issubclass(int, int))         # True
print(issubclass(list, list))       # True
print(issubclass(int, (str, int)))  # True

# ── constructors (zero-arg) ───────────────────────────────────────────
print(int())          # 0
print(str())          #
print(float())        # 0.0
print(bool())         # False
print(list())         # []
print(tuple())        # ()
print(dict())         # {}
print(set())          # set()
print(frozenset())    # frozenset()
print(bytes())        # b''
print(complex())      # 0j

# ── constructors (with argument) ──────────────────────────────────────
print(int("42"))             # 42
print(int(3.7))              # 3
print(str(123))              # 123
print(float("1.5"))          # 1.5
print(bool(0))               # False
print(bool(1))               # True
print(list((1, 2, 3)))       # [1, 2, 3]
print(tuple([1, 2, 3]))      # (1, 2, 3)
print(set([1, 2, 1]))        # {1, 2}
print(bytes(3))              # b'\x00\x00\x00'
print(complex(1, 2))         # (1+2j)
