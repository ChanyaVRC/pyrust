"""iter(OrderedDict) and its views report the "odict_iterator" type name.

CPython 3.12 returns an ``odict_iterator`` for the forward iterator of an
OrderedDict and of every one of its views (keys/values/items), unlike a plain
``dict`` which uses ``dict_keyiterator`` / ``dict_valueiterator`` /
``dict_itemiterator``.  Issue #2748.
"""

from collections import OrderedDict

od = OrderedDict([("a", 1), ("b", 2), ("c", 3)])

# Forward iterator of the OrderedDict itself.
print(type(iter(od)).__name__)
print(list(iter(od)))

# Forward iterators of the three views.
print(type(iter(od.keys())).__name__)
print(type(iter(od.values())).__name__)
print(type(iter(od.items())).__name__)

# Plain dict is unaffected.
print(type(iter(dict(a=1))).__name__)
print(type(iter(dict(a=1).keys())).__name__)
print(type(iter(dict(a=1).values())).__name__)
print(type(iter(dict(a=1).items())).__name__)

# A plain dict subclass (not OrderedDict) keeps the dict iterator name.
class PlainSub(dict):
    pass


print(type(iter(PlainSub(a=1))).__name__)

# An OrderedDict subclass inherits the odict iterator name.
class ODSub(OrderedDict):
    pass


print(type(iter(ODSub(a=1))).__name__)

# Empty OrderedDict still reports odict_iterator.
print(type(iter(OrderedDict())).__name__)

# Iteration order and the mutation guard remain correct.
od2 = OrderedDict([("x", 1), ("y", 2)])
it = iter(od2)
print(next(it))
od2["z"] = 3
try:
    next(it)
except RuntimeError as exc:
    print(str(exc))
