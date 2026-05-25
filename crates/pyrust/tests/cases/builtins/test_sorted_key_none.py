# sorted() and min()/max() with key=None should use natural ordering,
# not attempt to call None as a function.

# sorted: key=None is identical to omitting key
print(sorted([3, 1, 2], key=None))          # [1, 2, 3]
print(sorted([3, 1, 2]))                     # [1, 2, 3]
print(sorted([3, 1, 2], key=None, reverse=True))  # [3, 2, 1]

# sorted: non-None key function still works
print(sorted([3, 1, 2], key=lambda x: -x))  # [3, 2, 1]

# sorted: empty list with key=None
print(sorted([], key=None))                  # []

# sorted: strings
print(sorted('bca', key=None))              # ['a', 'b', 'c']

# min/max: key=None uses natural ordering
print(min([3, 1, 2], key=None))             # 1
print(max([3, 1, 2], key=None))             # 3
print(min([3, 1, 2]))                        # 1
print(max([3, 1, 2]))                        # 3
