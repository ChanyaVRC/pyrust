# list method CPython spec compliance
# Ref: https://docs.python.org/3/library/stdtypes.html#list.sort

# --- sort: key= callable must be applied ---

# key=len: sort strings by length
items = ['banana', 'apple', 'cherry', 'date']
items.sort(key=len)
print("sort-key-len", items)          # ['date', 'apple', 'banana', 'cherry']

# key=str.lower: case-insensitive sort
items2 = ['Banana', 'apple', 'Cherry']
items2.sort(key=str.lower)
print("sort-key-lower", items2)       # ['apple', 'Banana', 'Cherry']

# key=lambda: absolute value
nums = [-3, 1, -2, 4]
nums.sort(key=lambda x: abs(x))
print("sort-key-abs", nums)           # [1, -2, -3, 4]

# key= with reverse=True
items3 = ['banana', 'apple', 'cherry', 'date']
items3.sort(key=len, reverse=True)
print("sort-key-len-rev", items3)     # ['banana', 'cherry', 'apple', 'date']

# key= stability: equal keys preserve original order
pairs = [('b', 2), ('a', 2), ('c', 1)]
pairs.sort(key=lambda p: p[1])
print("sort-key-stable", pairs)       # [('c', 1), ('b', 2), ('a', 2)]

# sort with no key remains correct
nums2 = [3, 1, 4, 1, 5, 9, 2, 6]
nums2.sort()
print("sort-nokey", nums2)            # [1, 1, 2, 3, 4, 5, 6, 9]

# unknown kwarg must raise TypeError
try:
    [1, 2].sort(unknown=True)
    print("sort-unknown-kwarg", "no-error")
except TypeError:
    print("sort-unknown-kwarg", "TypeError")

# --- insert: large negative index clamps to 0 (prepend) ---

lst = [1, 2, 3]
lst.insert(-100, 'x')
print("insert-large-neg", lst)            # ['x', 1, 2, 3]

# large positive index clamps to len (append)
lst2 = [1, 2, 3]
lst2.insert(100, 'x')
print("insert-large-pos", lst2)           # [1, 2, 3, 'x']

# --- insert: non-integer index must raise TypeError ---

try:
    [1, 2, 3].insert('a', 99)
    print("insert-bad-type", "no-error")
except TypeError:
    print("insert-bad-type", "TypeError")
