d = {'a': 1, 'b': 2}

# All three views expose .mapping as a mappingproxy.
print(type(d.keys().mapping).__name__)
print(type(d.values().mapping).__name__)
print(type(d.items().mapping).__name__)

# hasattr sees it.
print(hasattr(d.keys(), 'mapping'))
print(hasattr(d.values(), 'mapping'))
print(hasattr(d.items(), 'mapping'))

# repr reflects the parent dict.
print(repr(d.keys().mapping))
print(repr(d.values().mapping))
print(repr(d.items().mapping))

# Content matches the dict.
print(dict(d.keys().mapping))
print({**d.values().mapping})

# Live updates are reflected (the proxy shares the dict's storage).
m = d.keys().mapping
d['c'] = 3
print('c' in m)
print(dict(m))

# Empty dict.
print(type({}.keys().mapping).__name__)
print(repr({}.values().mapping))
print(dict({}.items().mapping))

# Non-string keys round-trip through the proxy.
e = {1: 'x', (2, 3): 'y'}
mm = e.items().mapping
print(repr(mm))
print(mm[1], mm[(2, 3)])
print(list(mm))
print(len(mm))
print(mm == e)
print(mm.get(1), mm.get(99, 'default'))

# Read-only: mutation raises TypeError.
try:
    mm[5] = 'z'
except TypeError:
    print('readonly')

# Missing key raises KeyError.
try:
    mm[404]
except KeyError:
    print('keyerror')

# Mapping methods.
print(list(mm.keys()))
print(list(mm.values()))
print(type(mm.copy()).__name__, mm.copy())
