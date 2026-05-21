# Parity fixture for issue #893: set operations on slices with unhashable
# components should name the offending component, not 'slice'.
#
# CPython 3.12 behaviour: the error names the actual unhashable element
# inside the slice (e.g. 'list'), not the slice wrapper itself.

# ── set.update ────────────────────────────────────────────────────────────────

try:
    s = set()
    s.update([slice([1, 2], 3)])
except TypeError as e:
    print(e)  # unhashable type: 'list'

try:
    s = set()
    s.update([slice(1, [3, 4])])
except TypeError as e:
    print(e)  # unhashable type: 'list'

try:
    s = set()
    s.update([slice(1, 3, [1])])
except TypeError as e:
    print(e)  # unhashable type: 'list'

try:
    s = set()
    s.update([slice(1, {}, None)])
except TypeError as e:
    print(e)  # unhashable type: 'dict'

try:
    s = set()
    s.update([slice(1, set(), None)])
except TypeError as e:
    print(e)  # unhashable type: 'set'

# ── set.add ───────────────────────────────────────────────────────────────────

try:
    s = set()
    s.add(slice([1, 2], 3))
except TypeError as e:
    print(e)  # unhashable type: 'list'

# ── set.discard ───────────────────────────────────────────────────────────────

try:
    s = set()
    s.discard(slice([1, 2], 3))
except TypeError as e:
    print(e)  # unhashable type: 'list'

# ── set.remove ────────────────────────────────────────────────────────────────

try:
    s = set()
    s.remove(slice([1, 2], 3))
except TypeError as e:
    print(e)  # unhashable type: 'list'

# ── set() constructor ─────────────────────────────────────────────────────────

try:
    set([slice([1, 2], 3)])
except TypeError as e:
    print(e)  # unhashable type: 'list'

# ── frozenset() constructor ───────────────────────────────────────────────────

try:
    frozenset([slice([1, 2], 3)])
except TypeError as e:
    print(e)  # unhashable type: 'list'

# ── hash() builtin ────────────────────────────────────────────────────────────

try:
    hash(slice([1, 2], 3))
except TypeError as e:
    print(e)  # unhashable type: 'list'

# ── Nested: tuple-inside-slice (recursive leaf detection) ─────────────────────

try:
    s = set()
    s.update([slice(([1, 2], 3), 5)])
except TypeError as e:
    print(e)  # unhashable type: 'list'

# ── Happy paths: hashable slice and plain-list update ─────────────────────────

# All-integer slice is hashable.
s = set()
s.update([slice(1, 2, 3)])
print(len(s))  # 1

# None components are hashable.
s = set()
s.update([slice(None, 5)])
print(len(s))  # 1

# Plain-list item (not wrapped in slice) still reports 'list'.
try:
    s = set()
    s.update([[1, 2, 3]])
except TypeError as e:
    print(e)  # unhashable type: 'list'

# ── dict key path (value_to_pykey) ────────────────────────────────────────────

try:
    d = {}
    d[slice([1, 2], 3)] = 1
except TypeError as e:
    print(e)  # unhashable type: 'list'
