# collections cluster — issues #2010, #2013, #2011.
#
# #2010: Counter / defaultdict are real `dict` subclasses
#        (isinstance / issubclass / dict()-conversion / mapping pattern).
# #2013: Counter accepts keyword counts in __init__ / update, and has total().
# #2011: deque supports +, *, and reflected *.
#
# The parity harness diffs byte-for-byte against CPython 3.12, so every
# printed line must match.  We avoid printing dict-view / itertools objects
# (their repr embeds addresses) and class objects (the qualified-name prefix
# differs), printing only stable, structural values.
#
# Reference: https://docs.python.org/3/library/collections.html

import collections
from collections import Counter, defaultdict, deque

# ── #2010: dict-subclass relationship ────────────────────────────────────
print(isinstance(Counter(), dict))
print(isinstance(Counter("aab"), dict))
print(issubclass(Counter, dict))
print(isinstance(defaultdict(int), dict))
print(issubclass(defaultdict, dict))

# Counter / defaultdict are themselves still their own types.
print(type(Counter()).__name__)
print(type(defaultdict(int)).__name__)

# The first three entries of the MRO are (Self, dict, object).
print(Counter.__mro__[:3] == (Counter, dict, object))
print(defaultdict.__mro__[:3] == (defaultdict, dict, object))

# dict() conversion reads the backing mapping (was a ValueError before).
print(dict(Counter("aab")) == {"a": 2, "b": 1})
print(dict(Counter(a=2, b=1)) == {"a": 2, "b": 1})

dd = defaultdict(int)
dd["x"] += 2
dd["y"] += 1
print(dict(dd) == {"x": 2, "y": 1})

# Mapping match-pattern (PEP 634 gates on isinstance(_, Mapping)).
match Counter("aab"):
    case {"a": n}:
        print("matched", n)
    case _:
        print("no match")


# ── #2013: Counter kwargs + total ────────────────────────────────────────
print(dict(Counter(a=2, b=1)) == {"a": 2, "b": 1})
print(dict(Counter("ab", a=10)) == {"a": 11, "b": 1})

c = Counter()
c.update(x=5, y=2)
print(dict(c) == {"x": 5, "y": 2})

c2 = Counter("aab")
c2.update([1, 1, 2])
print(dict(c2) == {"a": 2, "b": 1, 1: 2, 2: 1})
c3 = Counter("aab")
c3.update({"a": 3})
print(dict(c3) == {"a": 5, "b": 1})

# subtract with kwargs (counts may go negative).
c4 = Counter(a=5)
c4.subtract(a=2, b=1)
print(sorted(c4.items()))

print(Counter("aabbb").total())
print(Counter().total())
print(Counter(a=-1, b=2).total())

# Positional mapping / iterable still works.
print(sorted(Counter([1, 1, 2]).items()))
print(sorted(Counter({"a": 2}).items()))


# ── #2011: deque +, *, reflected * ───────────────────────────────────────
print(deque([1, 2]) + deque([3, 4]) == deque([1, 2, 3, 4]))
print(deque([1, 2]) * 2 == deque([1, 2, 1, 2]))
print(2 * deque([1, 2]) == deque([1, 2, 1, 2]))
print(deque([1, 2]) * 0 == deque([]))
print(deque([1, 2]) * -1 == deque([]))
print(deque([1, 2]) * True == deque([1, 2]))

# maxlen: result inherits self's maxlen and is trimmed (rightmost kept).
r = deque([1, 2], maxlen=3) + deque([3, 4])
print(list(r), r.maxlen)
m = deque([1, 2, 3], maxlen=5) * 3
print(list(m), m.maxlen)

# Error paths.
try:
    deque([1, 2]) + [3, 4]
except TypeError as e:
    print("add", e)
try:
    deque([1, 2]) * 2.0
except TypeError as e:
    print("mul", e)
