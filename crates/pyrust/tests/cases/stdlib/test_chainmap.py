# collections.ChainMap — issue #1884.
#
# ChainMap is a view over several mappings: lookups search the maps in
# order, while writes / deletes only touch maps[0].  The parity harness
# asserts byte-identical output against CPython 3.12.
#
# Reference: https://docs.python.org/3/library/collections.html#collections.ChainMap

from collections import ChainMap

cm = ChainMap({'a': 1, 'b': 2}, {'b': 3, 'c': 4})

# lookup searches maps in order (first match wins)
print(cm['a'], cm['b'], cm['c'])
print(cm.get('a'), cm.get('z', 'default'))
print('a' in cm, 'z' in cm)
print(len(cm))

# iteration: first-occurrence order, last map first
print(list(cm))
print(dict(cm.items()))

# writes go to maps[0]
cm['x'] = 10
cm['b'] = 99
print(cm.maps[0])
print(cm['b'])

# new_child prepends a map
child = cm.new_child({'a': 100})
print(child['a'], child['b'])
print(child.new_child().maps[0])

# parents drops maps[0]
print(list(cm.parents.maps[0].items()))

# repr
print(repr(ChainMap({'a': 1})))
print(repr(ChainMap({'a': 1}, {'b': 2})))

# empty ChainMap defaults to a single empty dict
print(ChainMap().maps)

# fromkeys
print(ChainMap.fromkeys('ab', 0).maps)

# delete only from the first map
cm2 = ChainMap({'a': 1}, {'a': 2})
del cm2['a']
print(cm2['a'])
try:
    del cm2['nope']
except KeyError as e:
    print('del missing:', e)

# pop / popitem only from first map
cm3 = ChainMap({'a': 1, 'b': 2}, {'c': 3})
print(cm3.pop('a'))
print(cm3.maps[0])

# bool
print(bool(ChainMap({})), bool(ChainMap({'a': 1})))
