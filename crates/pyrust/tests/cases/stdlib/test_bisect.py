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

# ---- native-implementation surface (issue #3310) ----

import _bisect

# aliases are the very same function objects
print(bisect.bisect is bisect.bisect_right)
print(bisect.insort is bisect.insort_right)
print(bisect.bisect_left is _bisect.bisect_left, bisect.bisect_right is _bisect.bisect_right)
print(sorted(n for n in dir(bisect) if not n.startswith('_')))
print(hasattr(bisect, '__all__'))
print(bisect.__doc__)
print(bisect.bisect_left.__module__, bisect.insort_right.__name__)

# lo / hi bounds, keyword spellings, hi=-1 / hi=None mean len(a)
b = [10, 20, 30, 40, 50]
print(bisect.bisect_left(b, 35, 1, 3), bisect.bisect_right(b, 35, 1, 3))
print(bisect.bisect_left(b, 35, hi=None), bisect.bisect_right(b, 35, 0, -1))
print(bisect.bisect_left(b, 35, lo=5), bisect.bisect_left(b, 35, lo=10))
print(bisect.bisect_left(b, 35, 3, 1), bisect.bisect_right(b, 35, 0, -5))
print(bisect.bisect_left(a=b, x=30), bisect.bisect_right(x=30, a=b))
print(bisect.bisect_left(b, 30, True, 5))

# __index__-convertible bounds
class Idx:
    def __init__(self, n):
        self.n = n
    def __index__(self):
        return self.n

print(bisect.bisect_left(b, 30, Idx(1), Idx(4)))

# negative lo is a ValueError for every function (and insort leaves a untouched)
for fn in (bisect.bisect_left, bisect.bisect_right, bisect.insort_left, bisect.insort_right):
    try:
        fn(list(b), 25, lo=-1)
    except ValueError as e:
        print(fn.__name__, "ValueError:", e)
    try:
        fn(b, 25, -3, 2, key=None)
    except ValueError as e:
        print(fn.__name__, "ValueError:", e)

# bound argument validation
for args in ((b, 1, 1.5), (b, 1, 0, 2.5), (b, 1, "0"), (b, 1, 0, "x"), (b, 1, 2 ** 80), (b, 1, 0, 2 ** 80)):
    try:
        bisect.bisect_left(*args)
    except (TypeError, OverflowError) as e:
        print(type(e).__name__, e)
for call in (
    lambda: bisect.bisect_left(),
    lambda: bisect.bisect_left(b),
    lambda: bisect.bisect_left(b, 1, 0, None, None),
    lambda: bisect.bisect_left(b, 1, key=1),
    lambda: bisect.bisect_left(b, 1, bogus=1),
    lambda: bisect.insort_right(b, 1, key=5),
):
    try:
        call()
    except TypeError as e:
        print("TypeError:", e)

# generic sequences: only __len__ / __getitem__ are needed for bisect_*
class Seq:
    def __init__(self, data):
        self.data = data
        self.reads = []
    def __len__(self):
        return len(self.data)
    def __getitem__(self, i):
        self.reads.append(i)
        return self.data[i]

s = Seq([1, 2, 4, 4, 4, 8, 9])
print(bisect.bisect_left(s, 4), bisect.bisect_right(s, 4), s.reads)
print(bisect.bisect_left(s, 4, 1, 4), bisect.bisect_right(s, 100), bisect.bisect_left(s, -5))

# insort goes through .insert on non-list sequences
class InsertSeq(Seq):
    def insert(self, i, v):
        print("insert", i, v)
        self.data.insert(i, v)

t = InsertSeq([1, 3, 5])
bisect.insort(t, 4)
bisect.insort_left(t, 3)
bisect.insort_right(t, 3)
print(t.data)

try:
    bisect.insort(s, 3)
except AttributeError as e:
    print("AttributeError:", e)

# list subclasses are treated as generic sequences (overridden hooks are honoured)
class LoggedList(list):
    def insert(self, i, v):
        print("LoggedList.insert", i, v)
        super().insert(i, v)

ll = LoggedList([1, 5, 9])
bisect.insort(ll, 6)
print(ll)

class ShortLen(list):
    def __len__(self):
        return 2

print(bisect.bisect_left(ShortLen([1, 2, 3, 4]), 4))

# non-sequences
for bad in (5, None, {1: 2}, object()):
    try:
        bisect.bisect_left(bad, 1)
    except TypeError as e:
        print("TypeError:", e)

# hi beyond the sequence / sequence without __getitem__
try:
    bisect.bisect_left([1, 2], 5, 0, 10)
except IndexError as e:
    print("IndexError:", e)

class OnlyLen:
    def __len__(self):
        return 3

try:
    bisect.bisect_left(OnlyLen(), 1)
except TypeError as e:
    print("TypeError:", e)

# non-sequences are rejected up front, even when the search range is empty
class OnlyGet:
    def __getitem__(self, i):
        return i

for bad, extra in ((5, (0, 0)), (None, (0, 0)), (object(), (0, 0)), ({}, (0, 0)), ({1: 2}, (0, 1)),
                   (set(), ()), (OnlyLen(), ()), (OnlyGet(), (0, 0))):
    try:
        print(bisect.bisect_left(bad, 1, *extra))
    except TypeError as e:
        print("TypeError:", e)
try:
    bisect.insort_left(object(), 1, 0, 0, key=lambda v: v)
except TypeError as e:
    print("TypeError:", e)
print(bisect.bisect_left(OnlyGet(), 1, 0, 2))

# key= behaviour: applied to probed elements, never to x for bisect_*
calls = []
def k(v):
    calls.append(v)
    return v * 2

kd = [1, 2, 3, 4, 5]
print(bisect.bisect_left(kd, 6, key=k), calls)
del calls[:]
print(bisect.bisect_right(kd, 6, key=k), calls)
del calls[:]

# insort applies key to x first, then to the probed elements
kl = [1, 3, 5]
bisect.insort_left(kl, 2, key=k)
print(kl, calls)
del calls[:]
bisect.insort_right(kl, 3, key=k)
print(kl, calls)
bisect.insort(kl, 4, 0, 2, key=lambda v: v)
print(kl)
bisect.insort(kl, 0, key=None)
print(kl)

# key on records, descending key via negation
recs = [(5, 'e'), (3, 'c'), (1, 'a')]
print(bisect.bisect_left(recs, -3, key=lambda r: -r[0]))
bisect.insort_right(recs, (4, 'd'), key=lambda r: -r[0])
print(recs)

# exceptions from key / comparison / len / getitem propagate unchanged
class Boom(Exception):
    pass

def boom(v):
    raise Boom("key")

for fn in (bisect.bisect_left, bisect.insort_right):
    try:
        fn([1, 2, 3], 2, key=boom)
    except Boom as e:
        print(fn.__name__, "Boom", e)

class BadCmp:
    def __lt__(self, other):
        raise Boom("lt")
    def __gt__(self, other):
        raise Boom("gt")

try:
    bisect.bisect_left([BadCmp(), BadCmp()], BadCmp())
except Boom as e:
    print("Boom", e)

class BadLen:
    def __len__(self):
        raise Boom("len")

try:
    bisect.bisect_right(BadLen(), 1)
except Boom as e:
    print("Boom", e)

# only __lt__ is used: x < a[mid] for right, a[mid] < x for left
trace = []
class Tracer:
    def __init__(self, v):
        self.v = v
    def __lt__(self, other):
        trace.append(("lt", self.v, other.v))
        return self.v < other.v
    def __gt__(self, other):
        trace.append(("gt", self.v, other.v))
        return self.v > other.v

tl = [Tracer(1), Tracer(2), Tracer(3)]
print(bisect.bisect_left(tl, Tracer(2)), bisect.bisect_right(tl, Tracer(2)), trace)

# mixed numeric / str / tuple / big int data
print(bisect.bisect_left([1, 2.5, 4], 2.5), bisect.bisect_right([1, 2.5, 4], True))
print(bisect.bisect_left(["a", "c"], "b"), bisect.bisect_right([(1, 2), (1, 3)], (1, 2)))
print(bisect.bisect_left([2 ** 70, 2 ** 71], 2 ** 70 + 1))
try:
    bisect.bisect_left([1, "a"], 0.5)
except TypeError as e:
    print("TypeError:", e)

# non-list sequences: tuples, strings, ranges, bytes
print(bisect.bisect_left((1, 3, 5), 3), bisect.bisect_right("abcxyz", "c"), bisect.bisect_left(range(0, 100, 10), 35))
print(bisect.bisect_right(b"abcd", ord("b")), bisect.bisect_left(bytearray(b"abcd"), ord("d")))
try:
    bisect.insort((1, 2), 1)
except AttributeError as e:
    print("AttributeError:", e)

# the list is mutated between probes by a key function: result is still well formed
mut = [1, 2, 3, 4]
def mutating(v):
    if len(mut) > 3:
        mut.pop()
    return v
print(bisect.bisect_left(mut, 3, key=mutating))

# large sorted list stays sorted through repeated insort
import random as _r
_r.seed(7)
big = []
for _ in range(200):
    bisect.insort(big, _r.randrange(50))
print(big == sorted(big), len(big))
print("bisect native ok")
