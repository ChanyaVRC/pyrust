# Runtime TypeError for `**`-unpacking a mapping with non-string keys (issue
# #2146).  CPython raises `TypeError: keywords must be strings` rather than
# silently dropping the non-str entries.


def show(thunk):
    try:
        print(thunk())
    except TypeError as e:
        print("TypeError:", e)


def f(**kw):
    return kw


# Non-string keys must raise, not silently drop (#2146).
show(lambda: f(**{1: 2}))
show(lambda: dict(**{1: 2}))
show(lambda: dict(**{"a": 1, 2: 3}))


class A:
    def m(self, **kw):
        return kw


show(lambda: A().m(**{1: 2}))

# All-string-key unpacking is unchanged.
print(f(**{"a": 1, "b": 2}))
print(dict(**{"x": 1}))
print(A().m(**{"k": 5}))
print(f(**{"a": 1}, **{"b": 2}))
