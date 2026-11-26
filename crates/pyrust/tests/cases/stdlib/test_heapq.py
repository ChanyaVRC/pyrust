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

# heappop on empty raises IndexError with the C accelerator's wording
try:
    heapq.heappop([])
except IndexError as e:
    print("IndexError raised", e)

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

# ---------------------------------------------------------------------------
# Exact CPython 3.12 surface and error wording (heapq is 100% native, #3363)
# ---------------------------------------------------------------------------

print(heapq.__all__)
print([n for n in vars(heapq) if n in ("__about__", "__all__") or not n.startswith("__")])
print(heapq.__doc__.splitlines()[0])
print(heapq.__about__.splitlines()[0])
print(ascii(heapq.__about__.splitlines()[2]))


def show(call):
    try:
        print(call, "->", eval(call))
    except BaseException as e:
        print(call, "!!", type(e).__name__, e)


for call in [
    "heapq.heappush((), 1)",
    "heapq.heappush([])",
    "heapq.heappush([], 1, 2)",
    "heapq.heappush(heap=[], item=1)",
    "heapq.heappop(())",
    "heapq.heappop([])",
    "heapq.heappop()",
    "heapq.heappop([], [])",
    "heapq.heappop(heap=[])",
    "heapq.heapreplace([], 1)",
    "heapq.heapreplace((), 1)",
    "heapq.heapreplace([1])",
    "heapq.heapify(())",
    "heapq.heapify(1)",
    "heapq.heapify()",
    "heapq.heapify(x=[])",
    "heapq.heapify([])",
    "heapq.heappushpop((), 1)",
    "heapq.heappushpop([], 1)",
    "heapq.heappushpop([1])",
    "heapq._heappop_max([])",
    "heapq._heappop_max(3)",
    "heapq._heapreplace_max([], 1)",
    "heapq._heapreplace_max(1, 1)",
    "heapq._heapify_max(1)",
    "heapq._heapify_max([])",
    "heapq._siftdown([1], 0, 5)",
    "heapq._siftup([], 0)",
    "heapq._siftdown()",
    "heapq._siftup([1])",
    "heapq.nlargest('a', [1])",
    "heapq.nsmallest(2, 5)",
    "heapq.nlargest(2)",
    "heapq.nsmallest(n=1)",
    "heapq.nlargest(1, [], 2, 3)",
    "heapq.nlargest(2.5, [1, 2, 3, 4])",
    "heapq.nsmallest(None, [1, 2])",
    "heapq.nsmallest(0, [1])",
    "heapq.nlargest(-1, iter([1]))",
    "heapq.nlargest(1, iter([]))",
    "heapq.nsmallest(1, [], key=len)",
    "heapq.nsmallest(1, ['aa', 'b'], key=len)",
    "heapq.nlargest(1, iter(['aa', 'b']), key=len)",
    "heapq.nlargest(3, iter(range(10)), key=lambda x: -x)",
    "heapq.nsmallest(3, iter(range(10)), key=lambda x: -x)",
    "heapq.nsmallest(True, [3, 1])",
    "heapq.nlargest(1.0, [3, 1])",
    "heapq.nsmallest(10**30, [3, 1, 2])",
    "heapq.nlargest(10**30, iter([3, 1, 2]))",
    "heapq.nlargest(2, 'hello')",
    "heapq.nsmallest(2, {5: 0, 3: 0, 1: 0})",
    "heapq.nsmallest(2, (x for x in [9, 4, 7, 1]))",
    "heapq.nsmallest(2, range(100, 0, -1))",
    "heapq.nlargest(2, range(100))",
    "heapq.nlargest(5, [1, 1, 1, 1, 2])",
    "heapq.nlargest(3, [(1, 'a'), (1, 'b'), (2, 'a'), (0, 'z')], key=lambda t: t[0])",
    "heapq.nsmallest(3, [(1, 'a'), (1, 'b'), (2, 'a'), (0, 'z')], key=lambda t: t[0])",
    "list(heapq.merge(1))",
    "heapq.merge(1, key=1, reverse=1, x=2)",
    "list(heapq.merge())",
    "list(heapq.merge([], []))",
    "list(heapq.merge([1]))",
    "list(heapq.merge('abc', [1]))",
    "list(heapq.merge([1, 2], [1, 2], key=lambda x: x))",
    "list(heapq.merge([1, 3], [2, 4], key=lambda x: -x, reverse=True))",
    "list(heapq.merge([3, 1], [4, 2], reverse=True))",
]:
    show(call)

# The accelerated operations work on the list in place, whatever the subclass
# overrides, and reject non-lists.
x = [5, 3, 8, 1, 9, 2]
heapq.heapify(x)
print(x)
heapq._heapify_max(x)
print(x)
print(heapq._heappop_max(x), x)
print(heapq._heapreplace_max(x, 4), x)
print(heapq.heappushpop([1, 2], 0), heapq.heappushpop([1, 2], 5))


class L(list):
    def append(self, item):
        raise RuntimeError("no append")

    def pop(self, *args):
        raise RuntimeError("no pop")

    def __getitem__(self, index):
        raise RuntimeError("no getitem")


sub = L([3, 1, 2])
heapq.heapify(sub)
print(list.__repr__(sub))
heapq.heappush(sub, 0)
print(list.__repr__(sub))
print(heapq.heappop(sub))
print(list.__repr__(sub))


# Comparisons that resize the heap are detected.
class Grow:
    def __init__(self, v, lst):
        self.v = v
        self.lst = lst

    def __lt__(self, other):
        self.lst.append(0)
        return self.v < other.v


grow = []
heapq.heappush(grow, Grow(5, grow))
try:
    heapq.heappush(grow, Grow(3, grow))
except RuntimeError as e:
    print("RuntimeError", e)


class Clear:
    def __init__(self, v, lst):
        self.v = v
        self.lst = lst

    def __lt__(self, other):
        self.lst.clear()
        return self.v < other.v


for build in ([3, 1, 2], [1, 2]):
    lst = []
    lst.extend(Clear(v, lst) for v in build)
    try:
        heapq.heapify(lst)
        print("heapify ok", len(lst))
    except BaseException as e:
        print(type(e).__name__, e, len(lst))
lst = []
lst.extend([Clear(1, lst), Clear(2, lst)])
try:
    print(heapq.heappushpop(lst, Clear(5, lst)))
except BaseException as e:
    print(type(e).__name__, e)

# The pure-Python sift helpers work on any sequence.


class Seq:
    def __init__(self, items):
        self.items = items

    def __len__(self):
        return len(self.items)

    def __getitem__(self, i):
        return self.items[i]

    def __setitem__(self, i, v):
        self.items[i] = v


s = Seq([5, 4, 3, 2, 1])
heapq._siftup(s, 0)
print(s.items)
heapq._siftdown(s, 0, 4)
print(s.items)
heapq._siftup_max(s, 0)
print(s.items)
heapq._siftdown_max(s, 0, 3)
print(s.items)
y = [3, 1, 2]
heapq._siftdown(y, 0, 2)
print(y)
heapq._siftup(y, 0)
print(y)

# Tie-breaking and comparison order (instrumented values).
log = []


class T:
    def __init__(self, v):
        self.v = v

    def __lt__(self, other):
        log.append(("lt", self.v, other.v))
        return self.v < other.v

    def __eq__(self, other):
        log.append(("eq", self.v, other.v))
        return self.v == other.v

    def __hash__(self):
        return hash(self.v)

    def __repr__(self):
        return "T%d" % self.v


def flush(tag):
    print(tag, log)
    log.clear()


data = [5, 3, 9, 1, 7, 2, 8, 6, 4, 0]
h = [T(v) for v in data]
heapq.heapify(h)
flush("heapify")
heapq.heappush(h, T(-1))
flush("push")
print(heapq.heappop(h))
flush("pop")
print(heapq.heapreplace(h, T(3)))
flush("replace")
print(heapq.heappushpop(h, T(100)))
flush("pushpop")
print(heapq.heappushpop(h, T(-5)))
flush("pushpop-top")
heapq._heapify_max(h)
flush("heapify_max")
print(heapq._heappop_max(h))
flush("pop_max")
print(heapq._heapreplace_max(h, T(4)))
flush("replace_max")

a = [T(v) for v in (1, 4, 7, 7)]
b = [T(v) for v in (2, 4, 6, 8)]
c = [T(v) for v in (0, 5, 7)]
print(list(heapq.merge(a, b, c)))
flush("merge")
a = [T(v) for v in (7, 4, 1)]
b = [T(v) for v in (8, 4, 2)]
print(list(heapq.merge(a, b, reverse=True)))
flush("merge-reverse")
print(list(heapq.merge([1, 4, 7], [2, 4, 6, 8], key=T)))
flush("merge-key")
print(list(heapq.merge([7, 4, 1], [8, 4, 2], key=T, reverse=True)))
flush("merge-key-reverse")

# merge is lazy: nothing runs until the first next().


def gen(tag, items):
    for item in items:
        print("pull", tag, item)
        yield item


m = heapq.merge(gen("a", [1, 4]), gen("b", [2, 3]))
print("created")
print(next(m))
print(next(m))
print(next(m))
print(list(m))
print(list(m))

m = heapq.merge(5)
print("no error yet")
try:
    next(m)
except TypeError as e:
    print("TypeError", e)
print(list(m))
m = heapq.merge([1], [2])
print(iter(m) is m, hasattr(m, "__next__"), type(m).__name__)


# An error from an input stream ends the merge.
def bad():
    yield 1
    yield 3
    raise ValueError("boom")


m = heapq.merge(bad(), [2, 4, 5])
print(next(m), next(m), next(m))
try:
    next(m)
except ValueError as e:
    print("ValueError", e)
print(list(m))


def key_raises(x):
    if x == 3:
        raise KeyError(x)
    return x


m = heapq.merge([1, 3], [2, 4], key=key_raises)
try:
    print(list(m))
except KeyError as e:
    print("KeyError", e)
print(list(m))


def key_stops(x):
    if x == 3:
        raise StopIteration
    return x


# A StopIteration from the key function retires that stream.
print(list(heapq.merge([1, 3, 9], [2, 4], key=key_stops)))


def key_reenters(x):
    try:
        next(reentrant)
    except ValueError as e:
        print("re-entrant", e)
    return x


reentrant = heapq.merge([1, 2], [3], key=key_reenters)
print(list(reentrant))

print("heapq ok")
