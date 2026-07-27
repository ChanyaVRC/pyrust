import functools


def error_name(call):
    try:
        call()
    except Exception as exc:
        return type(exc).__name__
    return "NO_ERROR"


# Hashability follows normal dict/set semantics.  Failed key construction does
# not count as a cache miss because the wrapped function was never called.
@functools.lru_cache(maxsize=4)
def hashable_only(value):
    return len(value)


print("unhashable-list", error_name(lambda: hashable_only([])))
print("unhashable-dict", error_name(lambda: hashable_only({})))
print("unhashable-info", tuple(hashable_only.cache_info()))


# maxsize=0 is CPython's statistics-only pass-through and deliberately skips
# key construction, so unhashable arguments are accepted.
@functools.lru_cache(maxsize=0)
def passthrough(value):
    return len(value)


print("zero-unhashable", passthrough([1, 2]), tuple(passthrough.cache_info()))


class Key:
    def __init__(self, value):
        self.value = value

    def __hash__(self):
        return 17

    def __eq__(self, other):
        return isinstance(other, Key) and self.value == other.value

    def __repr__(self):
        # Equal repr is intentionally unrelated to equality.
        return "same-repr"


calls = [0]


@functools.lru_cache(maxsize=8)
def keyed(value):
    calls[0] += 1
    return calls[0]


print("equal-objects", keyed(Key(1)), keyed(Key(1)))
print("repr-collision", keyed(Key(2)), keyed(Key(3)))
print("object-info", tuple(keyed.cache_info()))


# Keyword order is part of CPython's key shape; it is not sorted.
keyword_calls = [0]


@functools.lru_cache(maxsize=8)
def keywords(**kwargs):
    keyword_calls[0] += 1
    return keyword_calls[0]


print("keyword-order", keywords(a=1, b=2), keywords(b=2, a=1))
print("keyword-info", tuple(keywords.cache_info()))


# Preserve CPython's one-argument int/str fast-key quirk at typed=False, while
# normal tuple-key equality still shares equal numeric values for multi-arg
# calls.  typed=True appends each immediate argument's exact type.
numeric_calls = [0]


@functools.lru_cache(maxsize=8)
def numeric(*args):
    numeric_calls[0] += 1
    return numeric_calls[0]


print("one-numeric", numeric(1), numeric(1.0), numeric(True))
print("many-numeric", numeric(2, 3), numeric(2.0, 3.0))
print("numeric-info", tuple(numeric.cache_info()))

typed_calls = [0]


@functools.lru_cache(maxsize=8, typed=True)
def typed(value):
    typed_calls[0] += 1
    return typed_calls[0]


print("typed-numeric", typed(1), typed(1.0), typed(True))
print("typed-info", tuple(typed.cache_info()))


# A hit promotes its entry to MRU.  In this sequence key 2, rather than the
# freshly-hit key 1, must be evicted when key 3 is inserted.
lru_calls = [0]


@functools.lru_cache(maxsize=2)
def bounded(value):
    lru_calls[0] += 1
    return lru_calls[0]


print(
    "lru-order",
    bounded(1),
    bounded(2),
    bounded(1),
    bounded(3),
    bounded(2),
)
print("lru-info", tuple(bounded.cache_info()))
bounded.cache_clear()
print("lru-clear", tuple(bounded.cache_info()))
