# Issue #3015: element-wise sequence operations use CPython's
# identity-before-equality rule.  An object must find itself without invoking
# equality, including generators and native iterators whose raw Value equality
# does not otherwise have an identity arm.


def source():
    yield 10
    yield 11


def make_generator():
    return source()


def make_list_iterator():
    return iter([10, 11])


def make_enumerate():
    return enumerate([10, 11])


def make_zip():
    return zip([10, 11], [20, 21])


def make_map():
    return map(lambda value: value + 1, [10, 11])


def make_filter():
    return filter(lambda value: value > 10, [10, 11])


def make_reversed():
    return reversed([10, 11])


def make_dict_values():
    return {1: 10, 2: 11}.values()


def index_result(container, target):
    try:
        return container.index(target)
    except ValueError:
        return "ValueError"


def remove_result(target):
    items = [target]
    try:
        items.remove(target)
        return len(items) == 0
    except ValueError:
        return "ValueError"


factories = (
    ("generator", make_generator, False),
    ("list_iterator", make_list_iterator, False),
    ("enumerate", make_enumerate, False),
    ("zip", make_zip, False),
    ("map", make_map, False),
    ("filter", make_filter, False),
    ("reversed", make_reversed, False),
    ("dict_values", make_dict_values, True),
)

for name, factory, is_view in factories:
    value = factory()
    print(
        name,
        value in [value],
        value in (value,),
        value not in [value],
        value not in (value,),
        [value] == [value],
        (value,) == (value,),
        [[value]] == [[value]],
        index_result([value], value),
        index_result((value,), value),
        [value].count(value),
        (value,).count(value),
        remove_result(value),
    )
    # None of the comparisons above may advance an iterator.  A dict_values
    # view is re-iterable rather than an iterator, so materialise that case.
    print(name, "state", list(value) if is_view else next(value))


# The generic iterable fallback behind `in` uses the same rule.  Finding the
# iterator as an element must not advance that iterator itself.
def singleton(value):
    yield value


iterator_target = iter([99])
print(
    "generator container",
    iterator_target in singleton(iterator_target),
    next(iterator_target),
)


class IterableBox:
    def __init__(self, value):
        self.value = value

    def __iter__(self):
        yield self.value


iterator_target = iter([98])
print(
    "instance iterable container",
    iterator_target in IterableBox(iterator_target),
    next(iterator_target),
)


class GetItemBox:
    def __init__(self, value):
        self.value = value

    def __getitem__(self, index):
        if index == 0:
            return self.value
        raise IndexError


iterator_target = iter([97])
print(
    "getitem container",
    iterator_target in GetItemBox(iterator_target),
    next(iterator_target),
)


# Plain primitives retain their borrow-only fast paths.
primitive_items = [1, 2, 3]
print(
    "primitive",
    2 in primitive_items,
    primitive_items == [1, 2, 3],
    primitive_items.index(2),
    primitive_items.count(2),
)


# A same-object NaN is found by containers even though bare equality remains
# false.  values_user_eq itself must stay identity-free.
nan = float("nan")
nan_items = [nan]
nan_removed = remove_result(nan)
print(
    "nan",
    nan == nan,
    nan in nan_items,
    nan_items == [nan],
    nan_items.index(nan),
    nan_items.count(nan),
    nan_removed,
)


events = []


class FalseEqual:
    def __eq__(self, other):
        events.append("false eq")
        return False


false_equal = FalseEqual()
print(
    "instance false",
    false_equal in [false_equal],
    [false_equal] == [false_equal],
    [false_equal].index(false_equal),
    [false_equal].count(false_equal),
    remove_result(false_equal),
    events,
)


class RaisingEqual:
    def __eq__(self, other):
        events.append("raising eq")
        raise RuntimeError("eq boom")


events.clear()
raising_equal = RaisingEqual()
print(
    "instance raising self",
    raising_equal in [raising_equal],
    [raising_equal] == [raising_equal],
    [raising_equal].index(raising_equal),
    [raising_equal].count(raising_equal),
    remove_result(raising_equal),
    events,
)

# Non-identical instances still dispatch __eq__ and propagate its exception.
try:
    RaisingEqual() in [RaisingEqual()]
    print("instance raising distinct no error")
except RuntimeError as error:
    print("instance raising distinct", str(error))
