# Parity fixture for Counter bool-value preservation (issue #930).
#
# CPython 3.12 preserves Bool/Int types when constructing a Counter from a
# mapping.  pyrust previously coerced all integer-valued counts to Int, losing
# the Bool type.
from collections import Counter

# --- construction from mapping preserves Bool ---
c = Counter({'a': True})
print(repr(c))            # Counter({'a': True})
print(type(c['a']))       # <class 'bool'>

# --- construction from mapping preserves Int ---
c2 = Counter({'a': 5})
print(type(c2['a']))      # <class 'int'>

# --- sorting in repr: Bool(True)=1 sorts below Int(2) ---
c3 = Counter({'a': True, 'b': 2})
print(repr(c3))           # Counter({'b': 2, 'a': True})

# --- most_common() preserves Bool type ---
print(c.most_common())              # [('a', True)]
print(type(c.most_common()[0][1])) # <class 'bool'>

# --- False (count=0) is preserved too ---
c4 = Counter({'a': False, 'b': 2})
print(repr(c4))           # Counter({'b': 2, 'a': False})
print(type(c4['a']))      # <class 'bool'>

# --- bool/int equality compatibility still holds ---
print(c['a'] == 1)        # True
print(c['a'] is True)     # True

# --- Counter arithmetic produces int (True + True = 2) ---
c5 = Counter({'a': True})
c6 = Counter({'a': True})
result = c5 + c6
print(result)             # Counter({'a': 2})
print(type(result['a']))  # <class 'int'>

# --- __setitem__ still allows Bool ---
c7 = Counter()
c7['x'] = True
print(type(c7['x']))      # <class 'bool'>
print(repr(c7))           # Counter({'x': True})

# --- mixed int and bool in one Counter ---
c8 = Counter({'a': True, 'b': False, 'c': 3})
print(type(c8['a']))      # <class 'bool'>
print(type(c8['b']))      # <class 'bool'>
print(type(c8['c']))      # <class 'int'>
