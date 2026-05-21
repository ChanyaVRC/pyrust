"""Parity fixture: obj[(a,b,c)] passes a tuple (not slice) to __getitem__ (issue #931).

Prior to the BuildSlice opcode, the VM's unpack_slice_key heuristic matched any
3-element tuple as a slice key.  This caused obj[(1,2,3)] to invoke __getitem__
with a slice object instead of a tuple for non-dict targets.
"""


# --- User-defined class ---
class M:
    def __getitem__(self, key):
        return f"{type(key).__name__}:{repr(key)}"


m = M()

# Tuple key: all lengths should pass a tuple.
print("1-tuple:", m[(1,)])
print("2-tuple:", m[(1, 2)])
print("3-tuple:", m[(1, 2, 3)])      # was broken: passed slice instead
print("inline 3:", m[1, 2, 3])       # same thing, inline syntax

# Slice key: should still pass a slice object.
print("slice 1:2:", m[1:2])
print("slice 1:2:3:", m[1:2:3])
print("slice :::", m[::])

# --- Built-in list: tuple subscript raises TypeError ---
lst = [0, 1, 2]
try:
    _ = lst[(1, 2, 3)]
except TypeError as e:
    print("list tuple key:", e)

try:
    _ = lst[1, 2]
except TypeError as e:
    print("list 2-tuple key:", e)

# List slice still works.
print("list slice:", lst[0:2])

# --- Dict: 3-element tuple is a valid key (not a slice) ---
d = {(1, 2, 3): "three"}
print("dict 3-tuple:", d[(1, 2, 3)])

# Dict with slice object as key (hashable slices).
d2 = {slice(1, 2): "s"}
print("dict slice key:", d2[slice(1, 2)])

# Dict slice notation: slice(1, 2, None) is not a key in d → KeyError.
try:
    _ = d[1:2]
except KeyError as e:
    print("dict slice notation: KeyError")

# --- Slice assignment and deletion ---
lst2 = [0, 1, 2, 3, 4]
lst2[1:3] = [10, 20]
print("slice assign:", lst2)

del lst2[1:2]
print("slice del:", lst2)

# Dict slice-key assignment and deletion.
d3 = {}
d3[slice(1, 3)] = 99
print("dict slice set:", d3[slice(1, 3)])
del d3[slice(1, 3)]
print("dict slice del empty:", len(d3) == 0)

# --- String ---
s = "hello"
print("str slice:", s[1:4])
try:
    _ = s[1, 2]
except TypeError:
    print("str tuple key: TypeError")
