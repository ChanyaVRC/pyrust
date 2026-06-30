import heapq
from heapq import heappush, heappop, heapify, heapreplace, heappushpop, nlargest, nsmallest, merge

# heapify
h = [5, 3, 1, 4, 2]
heapq.heapify(h)
print(h[0])  # 1

# heappush
heapq.heappush(h, 0)
print(h[0])  # 0

# heappop
print(heapq.heappop(h))  # 0
print(heapq.heappop(h))  # 1

# heappop drains in sorted order
h_sorted = [9, 7, 5, 3, 1, 8, 6, 4, 2, 0]
heapq.heapify(h_sorted)
print([heapq.heappop(h_sorted) for _ in range(10)])  # 0..9

# heappop on empty raises IndexError (message wording differs between
# CPython's C accelerator and the pure-Python fallback, so only assert
# the exception type)
try:
    heapq.heappop([])
except IndexError:
    print("IndexError raised")

# heapreplace
h2 = [1, 3, 5, 7, 9]
heapq.heapify(h2)
old = heapq.heapreplace(h2, 2)
print(old)  # 1
print(h2[0])  # 2

# heappushpop
h3 = [2, 4, 6]
heapq.heapify(h3)
result = heapq.heappushpop(h3, 1)
print(result)  # 1 (pushed 1, popped 1 because 1 < 2)
h4 = [2, 4, 6]
heapq.heapify(h4)
print(heapq.heappushpop(h4, 5))  # 2 (5 > 2 so old min comes out)

# nlargest/nsmallest
data = [5, 3, 1, 4, 2]
print(heapq.nlargest(3, data))   # [5, 4, 3]
print(heapq.nsmallest(3, data))  # [1, 2, 3]
print(heapq.nlargest(1, data))   # [5]
print(heapq.nsmallest(1, data))  # [1]
print(heapq.nlargest(0, data))   # []
print(heapq.nlargest(10, data))  # [5, 4, 3, 2, 1]
print(heapq.nsmallest(10, data))  # [1, 2, 3, 4, 5]

# with key (stable tie-break)
words = ['banana', 'apple', 'cherry', 'date']
print(heapq.nsmallest(2, words, key=len))  # ['date', 'apple']
print(heapq.nlargest(2, words, key=len))   # ['banana', 'cherry']

# merge
merged = list(heapq.merge([1, 3, 5], [2, 4, 6]))
print(merged)  # [1, 2, 3, 4, 5, 6]
print(list(heapq.merge([], [1, 2], [])))  # [1, 2]
print(list(heapq.merge()))  # []

# merge with key
data1 = [(1, 'a'), (3, 'c')]
data2 = [(2, 'b'), (4, 'd')]
merged2 = list(heapq.merge(data1, data2, key=lambda x: x[0]))
print(merged2)  # [(1,'a'),(2,'b'),(3,'c'),(4,'d')]

# merge reverse
print(list(heapq.merge([5, 3, 1], [6, 4, 2], reverse=True)))  # [6,5,4,3,2,1]

# build a heap via repeated push
hp = []
for v in [3, 1, 4, 1, 5, 9, 2, 6]:
    heapq.heappush(hp, v)
print([heapq.heappop(hp) for _ in range(len(hp))])  # sorted

print("heapq ok")
