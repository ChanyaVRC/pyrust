# Parity fixture for slice() builtin semantics (issues #848 and #850).
#
# Issue #848: slice() must reject keyword arguments with
#   TypeError: slice() takes no keyword arguments
#
# Issue #850: slice hashing — CPython 3.12 makes slice objects hashable
#   when all components are hashable.  PyInstance components must use
#   interpreter-level __hash__ dispatch (identity hash by default), not
#   silently fail.  A component with __hash__ = None makes the slice
#   unhashable with the component's type name in the error.

# --- keyword argument rejection (issue #848) ---

try:
    slice(stop=5)
except TypeError as e:
    print(type(e).__name__, str(e))   # TypeError slice() takes no keyword arguments

try:
    slice(1, stop=5)
except TypeError as e:
    print(type(e).__name__, str(e))   # TypeError slice() takes no keyword arguments

try:
    slice(start=1, stop=5)
except TypeError as e:
    print(type(e).__name__, str(e))   # TypeError slice() takes no keyword arguments

# --- positional forms continue to work ---

s1 = slice(5)
print(s1.start, s1.stop, s1.step)     # None 5 None

s2 = slice(1, 5)
print(s2.start, s2.stop, s2.step)     # 1 5 None

s3 = slice(1, 5, 2)
print(s3.start, s3.stop, s3.step)     # 1 5 2

# --- slice hashing — hashable components (issue #850) ---
# CPython 3.12 makes slice hashable when all components are hashable.

h1 = hash(slice(1, 2))
h2 = hash(slice(1, 2))
print(type(h1).__name__)    # int
print(h1 == h2)             # True — hash is stable within a session

# Equal slices hash equally
a = slice(1, 5, 2)
b = slice(1, 5, 2)
print(a == b)               # True
print(hash(a) == hash(b))   # True

# Slices with None components are hashable
print(type(hash(slice(None, None, None))).__name__)  # int
print(type(hash(slice(None, 10))).__name__)           # int

# Slices usable as dict keys
d = {slice(1, 2): "x", slice(3, 4): "y"}
print(d[slice(1, 2)])   # x
print(d[slice(3, 4)])   # y
print(len(d))           # 2

# Slice as set element
s_set = {slice(1, 2), slice(1, 2)}  # duplicates collapsed
print(len(s_set))       # 1

# --- slice hashing — instance component with __hash__ = None ---
# When a slice component is explicitly unhashable, hash(slice) raises TypeError.

class Unhashable:
    __hash__ = None

u = Unhashable()
try:
    hash(slice(u, 5))
except TypeError as e:
    print(type(e).__name__, str(e))   # TypeError unhashable type: 'Unhashable'

# --- slice hashing — instance component with custom __hash__ ---
# Instances with __hash__ defined are usable as slice components.

class HashThirty:
    def __hash__(self):
        return 30

obj = HashThirty()
s_inst = slice(obj, 5)
h_inst = hash(s_inst)
print(type(h_inst).__name__)   # int

# Same slice (same instance, same stop/step) hashes the same.
h_inst2 = hash(s_inst)
print(h_inst == h_inst2)       # True
