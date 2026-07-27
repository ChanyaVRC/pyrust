import collections
import contextlib
import itertools
import sys


@contextlib.contextmanager
def retained_manager(label):
    yield label


old_manager_factory = retained_manager
old_manager_type = contextlib._GeneratorContextManager

old_groupby = itertools.groupby([1, 2])
_, old_first_group = next(old_groupby)
old_grouper_type = type(old_first_group)

old_counter_type = collections.Counter
old_left = old_counter_type(a=3, b=1)
old_right = old_counter_type(a=1, c=2)


class OldCounterChild(old_counter_type):
    pass


old_child = OldCounterChild(a=2)

# Each fresh import publishes a new class generation. Retained factories and
# receivers must nevertheless keep resolving private siblings/base classes
# through the generation that owns them, rather than through the newest module.
del sys.modules["contextlib"]
del contextlib
import contextlib as new_contextlib

del sys.modules["itertools"]
del itertools
import itertools as new_itertools

del sys.modules["collections"]
del collections
import collections as new_collections


@new_contextlib.contextmanager
def new_manager(label):
    yield label


old_manager = old_manager_factory("old")
new_manager_instance = new_manager("new")
with old_manager as old_value:
    pass
with new_manager_instance as new_value:
    pass
print(
    "contextlib retained factory:",
    type(old_manager) is old_manager_type,
    type(old_manager) is new_contextlib._GeneratorContextManager,
    type(new_manager_instance) is new_contextlib._GeneratorContextManager,
    old_value,
    new_value,
)

new_groupby = new_itertools.groupby([3])
_, new_group = next(new_groupby)
_, old_second_group = next(old_groupby)
print(
    "itertools retained receiver:",
    type(old_second_group) is old_grouper_type,
    type(old_second_group) is type(new_group),
    list(old_second_group),
    list(new_group),
)

new_counter_type = new_collections.Counter
new_result = new_counter_type(x=1) + new_counter_type(x=2)
old_sum = old_left + old_right
old_difference = old_left - old_right
old_intersection = old_left & old_right
old_union = old_left | old_right
old_positive = +old_left
old_negative = -old_right
old_child_sum = old_child + old_right
print(
    "collections retained receiver:",
    type(old_sum) is old_counter_type,
    type(old_sum) is new_counter_type,
    all(
        type(result) is old_counter_type
        for result in (
            old_difference,
            old_intersection,
            old_union,
            old_positive,
            old_negative,
        )
    ),
    type(old_child_sum) is old_counter_type,
    type(old_child_sum) is OldCounterChild,
    type(new_result) is new_counter_type,
    sorted(old_sum.items()),
)
