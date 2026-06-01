# list.sort(key=None) must mean "no key function" (default comparison),
# matching CPython 3.12 and the existing sorted()/min()/max() behaviour.
# Regression test for #1937, where an explicit key=None was treated as a
# callable and raised TypeError: 'NoneType' object is not callable.

# key=None sorts by default comparison
l = [3, 1, 2]
l.sort(key=None)
print(l)

# key=None with reverse=True (stable)
l = [3, 1, 2]
l.sort(key=None, reverse=True)
print(l)

# stability of key=None on duplicates
l = [3, 1, 2, 1, 3, 2]
l.sort(key=None)
print(l)
l = [3, 1, 2, 1, 3, 2]
l.sort(key=None, reverse=True)
print(l)

# empty / single
l = []
l.sort(key=None)
print(l)
l = [5]
l.sort(key=None)
print(l)

# strings
l = ['banana', 'apple', 'cherry']
l.sort(key=None)
print(l)

# a real key function still works
l = ['bb', 'a', 'ccc']
l.sort(key=len)
print(l)

# non-callable, non-None key still raises TypeError
try:
    l = [3, 1, 2]
    l.sort(key=5)
    print(l)
except TypeError as e:
    print("TypeError:", e)

# sorted()/min()/max() with key=None remain correct
print(sorted([3, 1, 2], key=None))
print(min([3, 1, 2], key=None), max([3, 1, 2], key=None))
