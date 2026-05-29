import itertools

# zip_longest
print(list(itertools.zip_longest('AB', 'CD', 'E', fillvalue='-')))
print(list(itertools.zip_longest([1,2], [3], fillvalue=0)))

# filterfalse
print(list(itertools.filterfalse(None, [0, 1, False, 2, '', 'a'])))
print(list(itertools.filterfalse(lambda x: x % 2, range(6))))

# tee
a, b = itertools.tee([1, 2, 3])
print(list(a))
print(list(b))

# pairwise
print(list(itertools.pairwise('ABCD')))

# batched
print(list(itertools.batched('ABCDEFG', 3)))
