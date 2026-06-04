# collections.OrderedDict — issue #1884.
#
# OrderedDict is a dict subclass.  pyrust dicts are already insertion-
# ordered, so the order-aware surface is what matters: move_to_end,
# popitem(last=), order-sensitive __eq__, __reversed__, and the
# OrderedDict(...) repr.  The parity harness asserts byte-identical
# output against CPython 3.12.
#
# Reference: https://docs.python.org/3/library/collections.html#collections.OrderedDict

from collections import OrderedDict

od = OrderedDict()
od['a'] = 1
od['b'] = 2
od['c'] = 3
print(od)
print(list(od), list(od.keys()), list(od.values()), list(od.items()))
print(isinstance(od, dict))

# move_to_end
od.move_to_end('a')
print(list(od))
od.move_to_end('c', last=False)
print(list(od))

# popitem LIFO / FIFO
print(od.popitem())
print(od.popitem(last=False))

# reversed
print(list(reversed(OrderedDict([('a', 1), ('b', 2), ('c', 3)]))))

# order-sensitive equality: OD vs OD compares order, OD vs dict does not
print(OrderedDict([('a', 1), ('b', 2)]) == OrderedDict([('a', 1), ('b', 2)]))
print(OrderedDict([('a', 1), ('b', 2)]) == OrderedDict([('b', 2), ('a', 1)]))
print(OrderedDict([('a', 1), ('b', 2)]) == {'b': 2, 'a': 1})
print(OrderedDict([('a', 1)]) != OrderedDict([('a', 2)]))

# fromkeys / copy
print(OrderedDict.fromkeys('abc', 0))
print(OrderedDict([('a', 1), ('b', 2)]).copy())

# setdefault preserves order
od2 = OrderedDict([('a', 1)])
od2.setdefault('b', 2)
od2.setdefault('a', 99)
print(od2)

# union operators
print(OrderedDict([('a', 1)]) | {'b': 2})
print({'z': 9} | OrderedDict([('a', 1)]))
oz = OrderedDict([('a', 1)])
oz |= {'b': 2}
print(oz)

# repr
print(repr(OrderedDict()))
print(repr(OrderedDict([('a', 1)])))

# empty popitem
try:
    OrderedDict().popitem()
except KeyError as e:
    print('popitem empty:', e)

# move_to_end on a missing key
try:
    OrderedDict([('a', 1)]).move_to_end('z')
except KeyError as e:
    print('move missing:', e)
