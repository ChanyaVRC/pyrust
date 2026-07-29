# list.sort is keyword-only in CPython 3.12: sort(*, key=None, reverse=False).
# Any positional argument raises TypeError and leaves the list unchanged (#1949).


def try_sort(make, *args, **kwargs):
    lst = make()
    try:
        lst.sort(*args, **kwargs)
        print("sorted", lst)
    except TypeError as e:
        print("TypeError:", e, "| unchanged", lst)


# Positional args are rejected; list left unchanged.
try_sort(lambda: [3, 1, 2], None)
try_sort(lambda: [3, 1, 2], lambda x: -x)
try_sort(lambda: [3, 1, 2], 0)

# Keyword args still work.
try_sort(lambda: [3, 1, 2], key=None)
try_sort(lambda: [3, 1, 2], reverse=True)
try_sort(lambda: [3, 1, 2], key=lambda x: -x)
try_sort(lambda: [3, 1, 2], key=None, reverse=True)

# Bound method also rejects positional args.
m = [3, 1, 2].sort
try:
    m(None)
    print("bound sorted")
except TypeError as e:
    print("bound TypeError:", e)


# list subclass inherits the rejection.
class MyList(list):
    pass


sub = MyList([3, 1, 2])
try:
    sub.sort(None)
    print("subclass sorted")
except TypeError as e:
    print("subclass TypeError:", e, "| unchanged", list(sub))


# No-key user __lt__ dispatch still works (#1925).
class Item:
    def __init__(self, v):
        self.v = v

    def __lt__(self, other):
        return self.v < other.v


items = [Item(3), Item(1), Item(2)]
items.sort()
print("lt dispatch", [it.v for it in items])
