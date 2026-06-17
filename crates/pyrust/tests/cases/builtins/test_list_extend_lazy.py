# list.extend must accept any iterable, including lazy iterators (map / filter /
# zip / enumerate / generator expressions / user generators), not just eagerly
# materialised sequences and `iter([...])` (issue #2522).

# --- map object ---
r = []
r.extend(map(str, [1, 2, 3]))
print(r)  # ['1', '2', '3']

# --- generator expression ---
r = []
r.extend(x * 2 for x in [1, 2, 3])
print(r)  # [2, 4, 6]

# --- user generator function ---
def g():
    yield 10
    yield 20
r = []
r.extend(g())
print(r)  # [10, 20]

# --- filter object ---
r = []
r.extend(filter(lambda x: x > 1, [1, 2, 3]))
print(r)  # [2, 3]

# --- zip / enumerate yield tuples ---
r = []
r.extend(zip([1, 2], [3, 4]))
print(r)  # [(1, 3), (2, 4)]
r = []
r.extend(enumerate(["a", "b"]))
print(r)  # [(0, 'a'), (1, 'b')]

# --- empty lazy iterator leaves the receiver unchanged ---
r = [9]
r.extend(x for x in [])
print(r)  # [9]

# --- NativeIterFrame fast path (iter([...])) still works AND is exhausted ---
it = iter([1, 2, 3])
r = []
r.extend(it)
print(r, list(it))  # [1, 2, 3] []

# --- eager containers still work (regression guard) ---
r = []
r.extend([4, 5])
r.extend((6, 7))
r.extend("ab")
print(r)  # [4, 5, 6, 7, 'a', 'b']

# --- set / dict materialise through their iteration protocol ---
r = []
r.extend({1, 2, 3})
print(sorted(r))  # [1, 2, 3]
r = []
r.extend({"a": 1, "b": 2})
print(sorted(r))  # ['a', 'b']

# --- aliasing self-extend stays correct (#414) ---
a = [1, 2]
a.extend(a)
print(a)  # [1, 2, 1, 2]

# --- list subclass extends through its backing list ---
class MyList(list):
    pass
m = MyList([0])
m.extend(map(str, [1, 2]))
print(type(m).__name__, m)  # MyList [0, '1', '2']

# --- unbound list.extend with a lazy iterator ---
r = []
list.extend(r, (x for x in range(3)))
print(r)  # [0, 1, 2]

# --- non-iterable arguments raise TypeError with the type name ---
for bad in (None, 42, 3.14, True):
    try:
        [].extend(bad)
    except TypeError as e:
        print(type(bad).__name__, "->", e)

# --- wrong argument count matches CPython's TypeError wording ---
try:
    [].extend()
except TypeError as e:
    print(e)  # list.extend() takes exactly one argument (0 given)
try:
    [].extend(1, 2)
except TypeError as e:
    print(e)  # list.extend() takes exactly one argument (2 given)

# --- a generator that re-enters extend on the same generator raises the
#     CPython "generator already executing" ValueError ---
def selfconsume():
    yield 1
    out.extend(gen)
    yield 2
gen = selfconsume()
out = []
try:
    out.extend(gen)
    print(out)
except ValueError as e:
    print("ValueError:", e)  # generator already executing
