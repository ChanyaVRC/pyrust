d = {'a': 1}

# String key
try:
    _ = d['missing']
except KeyError as e:
    print(repr(e))      # KeyError('missing')
    print(str(e))       # 'missing'
    print(e.args[0])    # missing

# Integer key
try:
    _ = d[42]
except KeyError as e:
    print(repr(e))      # KeyError(42)
    print(str(e))       # 42
    print(e.args[0])    # 42

# Tuple key
try:
    _ = d[(1, 2)]
except KeyError as e:
    print(repr(e))      # KeyError((1, 2))
    print(str(e))       # (1, 2)
    print(e.args[0])    # (1, 2)

# dict.pop without default
try:
    d.pop('missing')
except KeyError as e:
    print(repr(e))      # KeyError('missing')
    print(str(e))       # 'missing'
    print(e.args[0])    # missing

# set.remove
s = {1, 2, 3}
try:
    s.remove(99)
except KeyError as e:
    print(repr(e))      # KeyError(99)
    print(str(e))       # 99
    print(e.args[0])    # 99

# dict.popitem on empty dict (message as string arg)
try:
    {}.popitem()
except KeyError as e:
    print(repr(e))      # KeyError('popitem(): dictionary is empty')
    print(e.args[0])    # popitem(): dictionary is empty

# set.pop on empty set (message as string arg)
try:
    set().pop()
except KeyError as e:
    print(repr(e))      # KeyError('pop from an empty set')
    print(e.args[0])    # pop from an empty set

# args[0] type checks
d2 = {}
try:
    d2['x']
except KeyError as e:
    print(type(e.args[0]).__name__)  # str

try:
    d2[42]
except KeyError as e:
    print(type(e.args[0]).__name__)  # int
