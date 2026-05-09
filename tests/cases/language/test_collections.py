# Lists, dicts, tuples, sets, slices, indexing, del, unpacking

# Lists
nums = [1, 2, 3]
print("list", nums)
print("index", nums[-1])
print("len", len(nums), len("abc"))

# Dictionaries
d = {"a": 1, "b": 2}
print("dict", d)
print("lookup", d["b"])
print("dict-len", len(d))

# Tuples
t = (1, 2, 3)
print("tuple", t)
print("tuple-idx", t[1])
single = (42,)
print("single-tuple", single)

# Slicing
lst = [0, 1, 2, 3, 4]
print("slice", lst[1:3])
print("slice-from", lst[2:])
print("slice-to", lst[:3])
print("str-slice", "hello"[1:4])
print("slice-step", lst[::2])
print("slice-rev", lst[::-1])

# Slice assignment
lst2 = [0, 1, 2, 3, 4]
lst2[1:3] = [9, 8, 7]
print("slice-assign", lst2)
lst3 = [0, 1, 2, 3, 4, 5]
lst3[::2] = [10, 20, 30]
print("slice-assign-step", lst3)

# Index assignment
lst = [10, 20, 30]
lst[1] = 99
print("list-assign", lst)

d = {"x": 1}
d["y"] = 2
print("dict-assign", d)

# Delete statement
d = {"a": 1, "b": 2}
del d["a"]
print("del-dict-key", d)

lst4 = [0, 1, 2, 3, 4, 5]
del lst4[1:4]
print("del-slice", lst4)

lst5 = [0, 1, 2, 3, 4, 5]
del lst5[::2]
print("del-slice-step", lst5)

# Unpacking assignment
a, b, c = 1, 2, 3
print("unpack", a, b, c)
a, b = b, a
print("swap", a, b)

# Unpack error messages (Issue #96)
try:
    a, b, c = (1, 2)
except Exception as e:
    print("unpack-too-few", str(e))

try:
    a, b = (1, 2, 3)
except Exception as e:
    print("unpack-too-many", str(e))

# Membership testing
lst = [1, 2, 3]
print("in-list", 2 in lst)
print("not-in-list", 5 not in lst)
print("in-str", "bc" in "abcd")
print("in-dict", "a" in {"a": 1})

# Sets
s = {1, 2, 3, 2, 1}
print("set-len", len(s))
print("set-in", 2 in s)
