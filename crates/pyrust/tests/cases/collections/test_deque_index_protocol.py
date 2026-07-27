"""Deque integer arguments share CPython's context-specific index protocol."""

from collections import deque


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


class BadIndex:
    def __index__(self):
        return "bad"


class IntSubclass(int):
    pass


def attempt(label, operation):
    try:
        print(label, "ok", operation())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


attempt("maxlen int subclass", lambda: deque([1, 2, 3], IntSubclass(2)))
attempt("maxlen index object", lambda: deque([1], Index(2)))
attempt("maxlen bigint", lambda: deque(maxlen=10**100))


values = deque([1, 2, 3])
values.rotate(Index(1))
print("rotate index", list(values))
values.rotate(IntSubclass(-1))
print("rotate int subclass", list(values))
attempt("rotate bigint", lambda: values.rotate(Index(10**100)))
attempt("rotate bad", lambda: values.rotate(BadIndex()))


values = deque([0, 1, 2, 1])
print("index bounds", values.index(1, Index(2), Index(4)))
print("index clipped", values.index(1, Index(-(10**100)), Index(10**100)))
attempt("index none", lambda: values.index(1, None))
attempt("index bad", lambda: values.index(1, BadIndex()))


values = deque([0, 1, 2])
values.insert(Index(-1), 9)
print("insert index", list(values))
attempt("insert bigint", lambda: values.insert(Index(10**100), 9))
full = deque([1], maxlen=1)
attempt("insert full bad", lambda: full.insert(BadIndex(), 2))


values = deque([0, 1, 2])
print("getitem index", values.__getitem__(Index(-1)))
values.__setitem__(Index(1), 9)
print("setitem index", list(values))
values.__delitem__(IntSubclass(0))
print("delitem int subclass", list(values))
attempt("getitem bigint", lambda: values.__getitem__(Index(10**100)))
attempt("setitem bad", lambda: values.__setitem__(BadIndex(), 3))


values = deque([1, 2], maxlen=5)
print("repeat index", list(values * Index(2)))
print("repeat int subclass", list(values * IntSubclass(2)))
attempt("repeat bigint", lambda: values * Index(10**100))
attempt("repeat bad", lambda: values * BadIndex())


values = deque([1, 2])


class MutatingIndex:
    def __index__(self):
        values.append(3)
        return -1


print("getitem mutation order", values.__getitem__(MutatingIndex()), list(values))
