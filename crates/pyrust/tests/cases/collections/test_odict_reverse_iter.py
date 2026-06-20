# issue #2741: reversed(OrderedDict) and reversed() over its views must report
# the `odict_iterator` type — the same type CPython 3.12 uses for the forward
# OrderedDict iterator — and iterate in reverse insertion order.
from collections import OrderedDict

d = OrderedDict([("a", 1), ("b", 2), ("c", 3)])

# reversed(OrderedDict) itself
print(type(reversed(d)).__name__)
print(list(reversed(d)))

# reversed() over each live view is also odict_iterator
print(type(reversed(d.keys())).__name__)
print(list(reversed(d.keys())))
print(type(reversed(d.values())).__name__)
print(list(reversed(d.values())))
print(type(reversed(d.items())).__name__)
print(list(reversed(d.items())))

# repr prefix
print(repr(reversed(d)).startswith("<odict_iterator"))

# empty OrderedDict
e = OrderedDict()
print(type(reversed(e)).__name__)
print(list(reversed(e)))

# single element
s = OrderedDict([("x", 10)])
print(type(reversed(s)).__name__)
print(list(reversed(s)))

# size-mutation guard still fires with OrderedDict wording
g = OrderedDict([("a", 1), ("b", 2), ("c", 3)])
it = reversed(g)
print(next(it))
g["z"] = 9
try:
    next(it)
except RuntimeError as exc:
    print("RuntimeError:", exc)

# clear() mid-iteration reports the "changed size" wording
h = OrderedDict([("a", 1), ("b", 2), ("c", 3)])
it2 = reversed(h)
print(next(it2))
h.clear()
try:
    next(it2)
except RuntimeError as exc:
    print("RuntimeError:", exc)
