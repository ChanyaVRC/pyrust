# --- split / rsplit pointer safety ---

# subslice offsets must be correct after maxsplit
s = "a b c d"
assert s.split(None, 2) == ["a", "b", "c d"], repr(s.split(None, 2))
assert s.rsplit(None, 2) == ["a b", "c", "d"], repr(s.rsplit(None, 2))

# separator split
assert "a::b::c".split("::") == ["a", "b", "c"]
assert "a::b::c".split("::", 1) == ["a", "b::c"]
assert "a::b::c".rsplit("::", 1) == ["a::b", "c"]

# empty string edge cases
assert "".split() == []
assert "  ".split() == []
assert "".split("x") == [""]

# --- str.split("") / rsplit("") must raise ValueError ---
ok = False
try:
    "hello".split("")
except ValueError:
    ok = True
assert ok, "split('') should raise ValueError"

ok = False
try:
    "hello".rsplit("")
except ValueError:
    ok = True
assert ok, "rsplit('') should raise ValueError"

# --- list.sort(key=fn) must raise NotImplementedError ---
ok = False
try:
    [3, 1, 2].sort(key=lambda x: x)
except NotImplementedError:
    ok = True
assert ok, "sort(key=fn) should raise NotImplementedError"

# sort without key still works
lst = [3, 1, 4, 1, 5]
lst.sort()
assert lst == [1, 1, 3, 4, 5]
lst.sort(reverse=True)
assert lst == [5, 4, 3, 1, 1]

print("split/sort OK")
