import itertools


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


class IntSubclass(int):
    # permutations must use the inherited integer backing, not this override.
    def __index__(self):
        return 1


class BadIndex:
    def __index__(self):
        return "2"


class Marker(Exception):
    pass


class RaisingIndex:
    def __index__(self):
        raise Marker("index called")


def show(label, thunk):
    try:
        print(label, thunk())
    except Exception as error:
        print(label, type(error).__name__, str(error))


# CPython 3.12 keeps permutations narrower than the other counted itertools
# constructors: r accepts real ints and int subclasses, but does not invoke an
# arbitrary object's __index__ method.
show("permutations-bool", lambda: list(itertools.permutations("abc", True)))
show(
    "permutations-int-subclass",
    lambda: list(itertools.permutations("abc", IntSubclass(2))),
)
show("permutations-none", lambda: list(itertools.permutations("ab", None)))
show("permutations-index", lambda: itertools.permutations("ab", Index(1)))
show("permutations-bad-index", lambda: itertools.permutations("ab", BadIndex()))
show(
    "permutations-raising-index",
    lambda: itertools.permutations("ab", RaisingIndex()),
)
show("permutations-negative", lambda: itertools.permutations("ab", -1))
show("permutations-overflow", lambda: itertools.permutations("", 1 << 100))


# The remaining counted constructors do use the general __index__ protocol.
show(
    "combinations-index",
    lambda: list(itertools.combinations("abc", Index(2))),
)
show(
    "combinations-with-replacement-index",
    lambda: list(itertools.combinations_with_replacement("ab", Index(2))),
)
show("repeat-index", lambda: list(itertools.repeat("x", Index(2))))
show(
    "product-index",
    lambda: list(itertools.product("ab", repeat=Index(2))),
)
show(
    "tee-index",
    lambda: [list(iterator) for iterator in itertools.tee("ab", Index(2))],
)
show("batched-index", lambda: list(itertools.batched("abc", Index(2))))
show(
    "islice-index",
    lambda: list(itertools.islice("abcde", Index(1), Index(5), Index(2))),
)
show(
    "combinations-bad-index",
    lambda: itertools.combinations("ab", BadIndex()),
)
show(
    "repeat-raising-index",
    lambda: itertools.repeat("x", RaisingIndex()),
)
