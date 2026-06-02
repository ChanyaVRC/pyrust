# Parity fixture for issue #2069: starred expressions inside a subscript
# (PEP 646, valid since CPython 3.11). The starred items are unpacked into the
# subscript index tuple; a single star still forces a 1-tuple index.

idx = (1, 2)
m = {(1, 2): "A", (1, 2, 3): "B"}

print(m[*idx])      # == m[1, 2]            -> 'A'
print(m[*idx, 3])   # == m[1, 2, 3]         -> 'B'
print(m[1, *[2]])   # == m[1, 2]            -> 'A'

# a single star still makes a tuple: m[*[1]] indexes with (1,)
single = {(1,): "x"}
print(single[*[1]])

# any star in the list forces a tuple even with leading plain elements
print(m[*[1, 2], 3])

# KeyError for a missing 1-tuple key matches CPython
try:
    m[*[1]]
except KeyError as e:
    print("KeyError", e)

# non-starred subscripts are unchanged
a = [10, 20, 30, 40]
print(a[1])
print(a[1, 2] if False else "skip-tuple-on-list")
print(a[1:3])
print(a[::2])
print(m[1, 2])
