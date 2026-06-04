# iter(x) returns x unchanged when x is already an iterator (#2117).
# An iterator's __iter__ must return self, so iter(it) is it == True.

# reversed() iterators over list / range / str / tuple.
for ctor in (
    lambda: reversed([1, 2, 3]),
    lambda: reversed(range(3)),
    lambda: reversed("abc"),
    lambda: reversed((1, 2)),
):
    it = ctor()
    print(iter(it) is it)  # True

# Other builtin iterators are their own __iter__ too.
e = enumerate([1, 2])
print(iter(e) is e)  # True
z = zip([1], [2])
print(iter(z) is z)  # True
m = map(str, [1])
print(iter(m) is m)  # True
f = filter(None, [1])
print(iter(f) is f)  # True
li = iter([1, 2, 3])
print(iter(li) is li)  # True
si = iter("ab")
print(iter(si) is si)  # True

# Non-iterator iterables build a *fresh* iterator each time.
dk = {1: 2}.keys()
print(iter(dk) is dk)  # False  (dict_keys is a view, not an iterator)
lst = [1, 2]
print(iter(lst) is lst)  # False
tup = (1, 2)
print(iter(tup) is tup)  # False

# Identity does not disturb iteration: iter(it) shares position with it.
r = reversed([10, 20, 30])
print(next(r))  # 30
r2 = iter(r)
print(next(r2))  # 20  (continues from same position)
print(list(r))  # [10]

# map / filter / zip / enumerate consume the *same* underlying iterator,
# because they call iter() on it and an iterator's __iter__ returns self.
# They must not eagerly drain or re-wrap a builtin iterator at construction.
src = reversed([10, 20, 30, 40])
next(src)  # drop 40
mp = map(lambda x: x, src)
print(next(mp))  # 30  (shares position with src)
print(next(src))  # 20  (map advanced src)
print(list(mp))  # [10]

e = enumerate([1, 2, 3, 4])
next(e)
fl = filter(lambda t: True, e)
print(next(fl))  # (1, 2)
print(next(e))  # (2, 3)  (filter advanced e)

# Constructing map over an iterator is lazy: it pulls nothing up front.
lazy = reversed([1, 2, 3])
_m = map(lambda x: x, lazy)
print(list(lazy))  # [3, 2, 1]  (map hasn't pulled yet, same object)
