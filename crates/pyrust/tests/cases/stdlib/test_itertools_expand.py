# itertools.compress — added in the #330 follow-up.
#
# Filters `data` by the corresponding `selectors` element; stops when
# either iterator is exhausted.

import itertools


# Basic case
print("compress-basic", list(itertools.compress("ABCDEF", [1, 0, 1, 0, 1, 1])))

# Both empty
print("compress-both-empty", list(itertools.compress([], [])))

# Empty data, non-empty selectors
print("compress-empty-data", list(itertools.compress([], [1, 2, 3])))

# Non-empty data, empty selectors
print("compress-empty-selectors", list(itertools.compress([1, 2, 3], [])))

# Selectors shorter than data — stop at selector exhaustion
print("compress-short-sel", list(itertools.compress([1, 2, 3, 4], [True, False, True])))

# Data shorter than selectors — stop at data exhaustion
print("compress-short-data", list(itertools.compress([1, 2], [True, True, True])))

# Mixed truthy/falsy selectors (0, empty string, None, non-empty string)
print("compress-mixed-truthy", list(itertools.compress([1, 2, 3, 4], [0, "", None, "x"])))

# All selected
print("compress-all-selected", list(itertools.compress([1, 2, 3], [True, True, True])))

# Generator as data source
print(
    "compress-generator-data",
    list(itertools.compress((x * 10 for x in range(1, 4)), [1, 0, 1])),
)

# Generator as selector source
def _sel():
    yield True
    yield False
    yield True


print("compress-generator-sel", list(itertools.compress([10, 20, 30], _sel())))

# Custom __bool__ in selector (must go through interpreter __bool__, not bare truthy)
class _Truthy:
    def __bool__(self):
        return True


class _Falsy:
    def __bool__(self):
        return False


print(
    "compress-custom-bool",
    list(itertools.compress([1, 2], [_Truthy(), _Falsy()])),
)


# Error: wrong argument count
try:
    itertools.compress([1], [2], [3])
except TypeError:
    print("compress-extra-arg TypeError")

try:
    itertools.compress([1])
except TypeError:
    print("compress-missing-arg TypeError")

# Error: stray keyword argument
try:
    itertools.compress([1], [1], bogus=True)
except TypeError:
    print("compress-bogus-kw TypeError")
