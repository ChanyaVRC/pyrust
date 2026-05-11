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

# --- list.sort(key=fn) ---
lst = [3, 1, 4, 1, 5]
lst.sort(key=lambda x: -x)
assert lst == [5, 4, 3, 1, 1], repr(lst)

lst = ["banana", "apple", "cherry"]
lst.sort(key=lambda s: len(s))
assert lst == ["apple", "banana", "cherry"], repr(lst)

# sort without key
lst = [3, 1, 4, 1, 5]
lst.sort()
assert lst == [1, 1, 3, 4, 5]
lst.sort(reverse=True)
assert lst == [5, 4, 3, 1, 1]

print("split/sort OK")

# sort/min/max TypeError for incompatible types — Issue #104
try:
    sorted([1, "a"])
except TypeError:
    print("sort-mixed-type", "TypeError")
try:
    min(1, "x")
except TypeError:
    print("min-mixed-type", "TypeError")
try:
    max([1, "a"])
except TypeError:
    print("max-mixed-type", "TypeError")
