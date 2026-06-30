import bisect
from bisect import bisect_left, bisect_right, insort, insort_left, insort_right

a = [1, 3, 5, 7, 9]
print(bisect.bisect_left(a, 5))   # 2
print(bisect.bisect_right(a, 5))  # 3
print(bisect.bisect(a, 5))        # 3 (alias for bisect_right)

# insertion points around the ends
print(bisect.bisect_left(a, 0))   # 0
print(bisect.bisect_right(a, 10))  # 5
print(bisect.bisect_left(a, 4))   # 2 (not present)

bisect.insort(a, 6)
print(a)  # [1, 3, 5, 6, 7, 9]

bisect.insort_left(a, 6)
print(a)  # [1, 3, 5, 6, 6, 7, 9]

# key= argument (Python 3.10+)
data = [(1, 'a'), (3, 'c'), (5, 'e')]
print(bisect.bisect_left(data, 3, key=lambda x: x[0]))   # 1
print(bisect.bisect_right(data, 3, key=lambda x: x[0]))  # 2

words = ['a', 'bb', 'ccc', 'dddd']
print(bisect.bisect_left(words, 3, key=len))  # 2

# insort with key
pairs = [(1, 'a'), (3, 'c')]
bisect.insort(pairs, (2, 'b'), key=lambda x: x[0])
print(pairs)  # [(1, 'a'), (2, 'b'), (3, 'c')]

# lo/hi bounds
a2 = [1, 2, 3, 4, 5]
print(bisect.bisect_left(a2, 3, lo=2, hi=4))   # 2
print(bisect.bisect_right(a2, 3, 0, 5))        # 3

# empty list
print(bisect.bisect_left([], 5))   # 0
print(bisect.bisect_right([], 5))  # 0

# duplicates
dup = [2, 2, 2, 2]
print(bisect.bisect_left(dup, 2))   # 0
print(bisect.bisect_right(dup, 2))  # 4

# negative lo raises ValueError
try:
    bisect.bisect_left(a2, 3, lo=-1)
except ValueError as e:
    print("ValueError:", e)

# the from-imports resolve to the same callables
print(bisect_left is bisect.bisect_left)
print(insort is insort_right)

print("bisect ok")
