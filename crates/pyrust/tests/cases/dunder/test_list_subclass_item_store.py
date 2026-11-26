"""`obj[key] = value` on list subclasses (found while fixing issue #3325).

Slice stores on a list subclass raised "list indices must be integers or
slices, not slice", which broke CPython's `socketserver._Threads.reap()`
(`self[:] = (generator)`), and a user `__setitem__` override was skipped for
`l[i] = v`.  Pins: the override sees every key type, and without an override
slice stores match plain-list semantics.
"""


def attempt(label, fn):
    try:
        fn()
        print(label, "ok")
    except (TypeError, ValueError) as exc:
        print(label, type(exc).__name__, exc)


class Plain(list):
    pass


p = Plain([1, 2, 3, 4, 5])
p[:] = [10, 20, 30, 40, 50]
print("replace all:", p, type(p).__name__)
p[1:3] = (x * 2 for x in range(3))
print("generator, grows:", p)
p[::2] = "abc"
print("extended slice:", p)
p[1:1] = iter([7, 8])
print("insert empty slice:", p)
p[-2:] = []
print("negative bounds, shrink:", p)
p[:] = p
print("self-assignment:", p)
attempt("non-iterable", lambda: p.__setitem__(slice(0, 1), 5))


def bad_extended():
    p[::2] = [1]


def bad_scalar():
    p[0:1] = 5


attempt("extended size mismatch", bad_extended)
attempt("scalar into slice", bad_scalar)
p[0] = "first"
p[-1] = "last"
print("int keys:", p)


class Logged(list):
    def __setitem__(self, key, value):
        print("  __setitem__", type(key).__name__, key if isinstance(key, int) else "", value)
        super().__setitem__(key, value)


q = Logged([1, 2, 3])
q[0] = "a"
q[1:2] = ["b", "c"]
q[::3] = ["y", "z"]
print("override:", q)


class Threads(list):
    """The shape of CPython's `socketserver._Threads`."""

    def reap(self):
        self[:] = (n for n in self if n % 2)

    def pop_all(self):
        self[:], result = [], self[:]
        return result


t = Threads(range(6))
t.reap()
print("reap:", t)
print("pop_all:", t.pop_all(), "left:", t, type(t).__name__)
