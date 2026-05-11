# dict method CPython spec compliance
# Ref: https://docs.python.org/3/library/stdtypes.html#dict.update

# --- update: accepts iterable of (key, value) pairs ---

d = {'a': 1}
d.update([('b', 2), ('c', 3)])
print("update-pairs-a", d['a'])       # 1
print("update-pairs-b", d['b'])       # 2
print("update-pairs-c", d['c'])       # 3
print("update-pairs-len", len(d))     # 3

# update with empty list (no-op)
d2 = {'x': 10}
d2.update([])
print("update-empty-pairs", d2['x']) # 10
print("update-empty-len", len(d2))   # 1

# update with no arguments (no-op)
d3 = {'y': 99}
d3.update()
print("update-noarg", d3['y'])        # 99

# update from another dict (basic case that was already working)
d4 = {'a': 1}
d4.update({'b': 2})
print("update-dict-a", d4['a'])       # 1
print("update-dict-b", d4['b'])       # 2

# update overwrites existing keys
d5 = {'a': 1, 'b': 2}
d5.update([('b', 99), ('c', 3)])
print("update-overwrite-b", d5['b']) # 99
print("update-overwrite-c", d5['c']) # 3

# --- keys/values/items: basic content ---

d6 = {'x': 1, 'y': 2}
print("keys-in", 'x' in d6.keys())        # True
print("keys-miss", 'z' in d6.keys())      # False
print("values-in", 1 in d6.values())      # True
print("items-in", ('x', 1) in d6.items()) # True

# --- keys/values/items: live view — reflects mutations after creation ---
# Returning a list snapshot will fail here because the mutation happens
# after the view is obtained.

d7 = {'a': 1}
ks = d7.keys()
d7['b'] = 2
print("keys-view-live", 'b' in ks)        # True

d8 = {'x': 1}
vs = d8.values()
d8['x'] = 99
print("values-view-live", 99 in vs)       # True

d9 = {'a': 1}
it = d9.items()
d9['b'] = 2
print("items-view-live", ('b', 2) in it)  # True
