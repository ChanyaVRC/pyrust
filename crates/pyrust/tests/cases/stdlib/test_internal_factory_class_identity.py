import functools
import itertools
import sys


def compare(left, right):
    return left - right


key_factory = functools.cmp_to_key(compare)
key_one = key_factory(1)
key_two = key_factory(2)
another_factory = functools.cmp_to_key(compare)
print(
    "cmp identities:",
    type(key_one) is type(key_two),
    type(key_factory) is type(another_factory),
    type(key_one).__module__,
    type(key_factory).__module__,
)


@functools.lru_cache()
def first(value):
    return value


@functools.lru_cache()
def second(value):
    return value


print(
    "lru identities:",
    type(first) is type(second),
    type(first).__module__,
    functools.partial.__module__,
)

grouped = itertools.groupby([1, 2])
_, first_group = next(grouped)
_, second_group = next(grouped)
print(
    "grouper identities:",
    type(first_group) is type(second_group),
    type(first_group).__module__,
)

# Removing a module from sys.modules creates a fresh PyRust module generation.
# Factories in that generation must still share one class, while objects from
# the old generation retain their original class and behaviour.
old_key_type = type(key_one)
old_lru_type = type(first)
del sys.modules["functools"]
import functools as reloaded_functools

reloaded_factory = reloaded_functools.cmp_to_key(compare)
reloaded_one = reloaded_factory(1)
reloaded_two = reloaded_factory(2)


@reloaded_functools.lru_cache()
def reloaded_first(value):
    return value


@reloaded_functools.lru_cache()
def reloaded_second(value):
    return value


print(
    "functools reimport:",
    type(reloaded_one) is type(reloaded_two),
    type(reloaded_first) is type(reloaded_second),
    type(reloaded_one).__module__,
    type(reloaded_first).__module__,
)
print(
    "functools old alive:",
    type(key_one) is old_key_type,
    type(first) is old_lru_type,
    key_one < key_two,
    first(7),
)

old_group_type = type(first_group)
del sys.modules["itertools"]
import itertools as reloaded_itertools

reloaded_groups = reloaded_itertools.groupby([3, 4])
_, reloaded_group_one = next(reloaded_groups)
_, reloaded_group_two = next(reloaded_groups)
print(
    "itertools reimport:",
    type(reloaded_group_one) is type(reloaded_group_two),
    type(reloaded_group_one).__module__,
)
print(
    "itertools old alive:",
    type(first_group) is old_group_type,
    list(first_group),
)
