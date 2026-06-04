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
