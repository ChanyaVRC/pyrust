# copy module — parity fixture for copy.copy and copy.deepcopy.
#
# Covers: shallow copy independence, deep copy full independence,
# immutable pass-through, and PyInstance with/without __copy__/__deepcopy__.

import copy

# ── copy.copy: immutable types return the same object ────────────────────────

assert copy.copy(42) == 42
assert copy.copy(3.14) == 3.14
assert copy.copy("hello") == "hello"
assert copy.copy(True) is True
assert copy.copy(None) is None
assert copy.copy((1, 2, 3)) == (1, 2, 3)
assert copy.copy(b"bytes") == b"bytes"
assert copy.copy(frozenset({1, 2})) == frozenset({1, 2})

# ── copy.copy: mutable containers produce independent top-level copies ────────

orig_list = [1, 2, 3]
shallow = copy.copy(orig_list)
assert shallow == [1, 2, 3]
assert shallow is not orig_list

shallow.append(4)
assert orig_list == [1, 2, 3]   # original unaffected

orig_dict = {"a": 1, "b": 2}
shallow_dict = copy.copy(orig_dict)
assert shallow_dict == {"a": 1, "b": 2}
assert shallow_dict is not orig_dict

shallow_dict["c"] = 3
assert "c" not in orig_dict   # original unaffected

orig_set = {1, 2, 3}
shallow_set = copy.copy(orig_set)
assert shallow_set == {1, 2, 3}
assert shallow_set is not orig_set

shallow_set.add(4)
assert 4 not in orig_set   # original unaffected

# ── copy.copy: nested list — shallow means inner list is still shared ─────────

inner = [10, 20]
outer = [inner, "x"]
shallow_outer = copy.copy(outer)
assert shallow_outer == [[10, 20], "x"]
assert shallow_outer is not outer
assert shallow_outer[0] is inner   # inner list is shared (shallow!)

# ── copy.deepcopy: nested list — fully independent ────────────────────────────

deep_outer = copy.deepcopy(outer)
assert deep_outer == [[10, 20], "x"]
assert deep_outer is not outer
assert deep_outer[0] is not inner  # inner list is a new object

deep_outer[0].append(30)
assert inner == [10, 20]           # original inner list unaffected

# ── copy.deepcopy: nested dict ────────────────────────────────────────────────

nested_dict = {"x": {"y": 99}}
deep_dict = copy.deepcopy(nested_dict)
assert deep_dict == {"x": {"y": 99}}
deep_dict["x"]["y"] = 0
assert nested_dict["x"]["y"] == 99  # original unaffected

# ── copy.deepcopy: immutable types ───────────────────────────────────────────

assert copy.deepcopy(42) == 42
assert copy.deepcopy("hi") == "hi"
assert copy.deepcopy((1, 2)) == (1, 2)

# ── copy.deepcopy: tuple element recursion ────────────────────────────────────

t = ([1, 2], [3, 4])
deep_t = copy.deepcopy(t)
assert deep_t == ([1, 2], [3, 4])
deep_t[0].append(99)
assert t[0] == [1, 2]  # original unaffected

# ── PyInstance without __copy__ / __deepcopy__ ────────────────────────────────

class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

p = Point(1, 2)
p_shallow = copy.copy(p)
assert p_shallow.x == 1
assert p_shallow.y == 2
assert p_shallow is not p

p_shallow.x = 99
assert p.x == 1  # original unaffected

p_deep = copy.deepcopy(p)
assert p_deep.x == 1
assert p_deep.y == 2
assert p_deep is not p

# ── PyInstance with __copy__ ──────────────────────────────────────────────────

class WithCopy:
    def __init__(self, val):
        self.val = val
        self.copied = False

    def __copy__(self):
        obj = WithCopy(self.val)
        obj.copied = True
        return obj

wc = WithCopy(42)
wc_copy = copy.copy(wc)
assert wc_copy.val == 42
assert wc_copy.copied is True   # __copy__ was called
assert wc.copied is False

# ── PyInstance with __deepcopy__ ──────────────────────────────────────────────

class WithDeepCopy:
    def __init__(self, val):
        self.val = val
        self.deep_copied = False

    def __deepcopy__(self, memo):
        obj = WithDeepCopy(self.val)
        obj.deep_copied = True
        return obj

wd = WithDeepCopy(7)
wd_deep = copy.deepcopy(wd)
assert wd_deep.val == 7
assert wd_deep.deep_copied is True   # __deepcopy__ was called
assert wd.deep_copied is False

print("copy ok")
