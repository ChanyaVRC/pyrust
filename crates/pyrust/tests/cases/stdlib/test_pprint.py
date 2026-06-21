import pprint

# Basic pformat — short collections stay on one line
print(pprint.pformat([1, 2, 3]))
print(pprint.pformat({'a': 1, 'b': 2}))
print(pprint.pformat((1, 2, 3)))
print(pprint.pformat((1,)))
print(pprint.pformat({1, 2, 3}))
print(pprint.pformat('hello'))
print(pprint.pformat(42))
print(pprint.pformat(None))
print(pprint.pformat(3.5))
print(pprint.pformat(b'hello'))
print(pprint.pformat([]))
print(pprint.pformat({}))
print(pprint.pformat(()))

# Collections that fit on one line vs. those that wrap
print(pprint.pformat(list(range(10))))
print(pprint.pformat(list(range(30))))

# Nested structures
print(pprint.pformat({'a': [1, 2, 3], 'b': {'c': 4}}))

# Custom indent via PrettyPrinter
pp = pprint.PrettyPrinter(indent=2)
print(pp.pformat({'key': [1, 2, 3]}))
print(pprint.PrettyPrinter(indent=4).pformat({'key': list(range(20))}))

# width tuning forces wrapping
print(pprint.pformat([1, 2, 3, 4, 5], width=10))

# Depth limiting
nested = {'a': {'b': {'c': 'd'}}}
print(pprint.pformat(nested, depth=2))
print(pprint.pformat([1, [2, [3, [4]]]], depth=2))

# sort_dicts toggling
print(pprint.pformat({'b': 1, 'a': 2}, sort_dicts=False))
print(pprint.pformat({'b': 1, 'a': 2}, sort_dicts=True))

# compact mode
print(pprint.pformat(list(range(20)), compact=True, width=30))

# underscore_numbers
print(pprint.pformat(1234567, underscore_numbers=True))

# pprint() prints to stdout with trailing newline
pprint.pprint([1, 2, 3])
pprint.pprint({'x': 1})

# pp() — does not sort dicts by default
pprint.pp({'b': 1, 'a': 2})

# saferepr
print(pprint.saferepr({'a': 1, 'b': [1, 2]}))

# isreadable / isrecursive
print(pprint.isreadable({'a': 1}))
print(pprint.isreadable([1, 2, 3]))
print(pprint.isrecursive([1, 2]))

# Recursive structures — `id()` in the recursion marker is non-deterministic,
# so assert on the flags and the marker shape rather than the raw repr.
a = []
a.append(a)
print(pprint.isrecursive(a))
print(pprint.isreadable(a))
ra = pprint.pformat(a)
print(ra.startswith('[<Recursion on list with id='), ra.endswith('>]'))

d = {}
d['self'] = d
print(pprint.isrecursive(d))
rd = pprint.pformat(d)
print(rd.startswith("{'self': <Recursion on dict with id="), rd.endswith('>}'))

# A short dataclass fits on one line, so both CPython and pyrust emit the
# plain repr (pyrust does not yet special-case wrapping dataclasses).
import dataclasses


@dataclasses.dataclass
class Point:
    x: int
    y: int


print(pprint.pformat(Point(1, 2)))

# Error cases
try:
    pprint.PrettyPrinter(indent=-1)
except ValueError as e:
    print('ValueError:', e)
try:
    pprint.PrettyPrinter(depth=0)
except ValueError as e:
    print('ValueError:', e)
try:
    pprint.PrettyPrinter(width=0)
except ValueError as e:
    print('ValueError:', e)

print("pprint ok")
