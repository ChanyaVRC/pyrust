# Starred expressions in return and yield statements (issue #585).
# CPython 3.12 treats `return *a, b` as `return (*a, b)`.

a = [1, 2]


def f():
    return *a, 3


print(f())  # (1, 2, 3)


def g():
    return 0, *a, 3


print(g())  # (0, 1, 2, 3)


def h():
    return *[1, 2], *[3, 4]


print(h())  # (1, 2, 3, 4)


# Trailing-comma form: *a, is still a tuple.
def t():
    return *a,


print(t())  # (1, 2)


# yield variant.
def gen():
    yield *[1, 2], 3


print(list(gen()))  # [(1, 2, 3)]
