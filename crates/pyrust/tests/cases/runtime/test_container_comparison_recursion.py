from collections import deque


def outcome(label, operation):
    try:
        print(label, "value", operation())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


def cyclic_list():
    value = []
    value.append(value)
    return value


def cyclic_dict():
    value = {}
    value["self"] = value
    return value


def cyclic_deque():
    value = deque()
    value.append(value)
    return value


def mutual_list_cycle():
    first = []
    second = []
    first.append(second)
    second.append(first)
    return first


def mutual_dict_cycle():
    first = {}
    second = {}
    first["self"] = second
    second["self"] = first
    return first


def mutual_deque_cycle():
    first = deque()
    second = deque()
    first.append(second)
    second.append(first)
    return first


def compare_distinct(factory, operation):
    left = factory()
    right = factory()
    return left == right if operation == "eq" else left != right


for container_name, container_factory in (
    ("list", cyclic_list),
    ("dict", cyclic_dict),
    ("deque", cyclic_deque),
    ("mutual-list", mutual_list_cycle),
    ("mutual-dict", mutual_dict_cycle),
    ("mutual-deque", mutual_deque_cycle),
):
    for comparison in ("eq", "ne"):
        outcome(
            container_name + " distinct " + comparison,
            lambda factory=container_factory, operation=comparison: compare_distinct(
                factory, operation
            ),
        )
    same = container_factory()
    outcome(container_name + " identity eq", lambda same=same: same == same)
    outcome(container_name + " identity ne", lambda same=same: same != same)


def compare_ordering(factory, operation):
    left = factory()
    right = factory()
    if operation == "lt":
        return left < right
    if operation == "le":
        return left <= right
    if operation == "gt":
        return left > right
    return left >= right


for ordering_container, ordering_factory in (
    ("list", cyclic_list),
    ("deque", cyclic_deque),
):
    for ordering_operation in ("lt", "le", "gt", "ge"):
        outcome(
            ordering_container + " distinct " + ordering_operation,
            lambda factory=ordering_factory,
            operation=ordering_operation: compare_ordering(factory, operation),
        )


def scan(operation, sequence_factory, needle_factory):
    sequence = sequence_factory((needle_factory(),))
    needle = needle_factory()
    if operation == "contains":
        return needle in sequence
    if operation == "index":
        return sequence.index(needle)
    if operation == "count":
        return sequence.count(needle)
    return sequence.remove(needle)


for scan_container_name, scan_container_factory in (
    ("list", list),
    ("deque", deque),
):
    for needle_name, needle_factory in (
        ("list", cyclic_list),
        ("dict", cyclic_dict),
        ("deque", cyclic_deque),
    ):
        for scan_operation in ("contains", "index", "count", "remove"):
            outcome(
                scan_container_name
                + " cyclic "
                + needle_name
                + " element "
                + scan_operation,
                lambda operation=scan_operation,
                sequence_factory=scan_container_factory,
                factory=needle_factory: scan(operation, sequence_factory, factory),
            )


# Seeing the same structural pair again is not sufficient to reject the
# comparison: an element callback may make observable progress and remove the
# recursive edge before the next pass reaches it.
def finite_callback_cycle(kind, operation):
    calls = [0]
    left = None
    right = None

    class Progress:
        def __eq__(self, other):
            calls[0] += 1
            if calls[0] == 3:
                if kind in ("list", "deque"):
                    left[1] = 0
                    right[1] = 0
                else:
                    left["self"] = 0
                    right["self"] = 0
            return True

    if kind == "list":
        left = []
        right = []
        left.extend((Progress(), left))
        right.extend((Progress(), right))
    elif kind == "deque":
        left = deque()
        right = deque()
        left.extend((Progress(), left))
        right.extend((Progress(), right))
    else:
        left = {"probe": Progress()}
        right = {"probe": Progress()}
        left["self"] = left
        right["self"] = right

    if operation == "eq":
        result = left == right
    elif operation == "ne":
        result = left != right
    elif operation == "lt":
        result = left < right
    elif operation == "le":
        result = left <= right
    elif operation == "gt":
        result = left > right
    else:
        result = left >= right
    return result, calls[0]


for callback_container in ("list", "dict", "deque"):
    callback_operations = (
        ("eq", "ne")
        if callback_container == "dict"
        else ("eq", "ne", "lt", "le", "gt", "ge")
    )
    for callback_operation in callback_operations:
        outcome(
            callback_container + " finite callback " + callback_operation,
            lambda kind=callback_container,
            operation=callback_operation: finite_callback_cycle(kind, operation),
        )


def finite_callable_callback_cycle(descriptor=False):
    calls = [0]
    left = []
    right = []

    class EqCallable:
        def __call__(self, other):
            calls[0] += 1
            if calls[0] == 3:
                left[1] = 0
                right[1] = 0
            return True

    class EqDescriptor:
        def __get__(self, instance, owner):
            return EqCallable()

    class Item:
        pass

    Item.__eq__ = EqDescriptor() if descriptor else EqCallable()

    left.extend((Item(), left))
    right.extend((Item(), right))
    return left == right, calls[0]


outcome("list callable finite callback eq", finite_callable_callback_cycle)
outcome(
    "list descriptor finite callback eq",
    lambda: finite_callable_callback_cycle(descriptor=True),
)


class CallableForever:
    calls = 0

    def __call__(self, other):
        type(self).calls += 1
        return True


class CallableItem:
    __eq__ = CallableForever()


def callable_progress_exhaustion():
    left = []
    right = []
    left.extend((CallableItem(), left))
    right.extend((CallableItem(), right))
    try:
        return left == right
    except Exception as exc:
        return type(exc).__name__, str(exc), CallableForever.calls


print("list callable progress exhaustion", callable_progress_exhaustion())
outcome("list callable cleanup", finite_callable_callback_cycle)


def finite_flat_builtin_callback_cycle(break_at, operation):
    calls = [0]
    left = []
    right = []

    class Item:
        __eq__ = next

        def __next__(self):
            calls[0] += 1
            if calls[0] == break_at:
                left[1] = 0
                right[1] = 0
            return True

    left.extend((Item(), left))
    right.extend((Item(), right))
    result = left == right if operation == "eq" else left != right
    return result, calls[0]


for flat_builtin_break_at in (3, 100):
    for flat_builtin_operation in ("eq", "ne"):
        outcome(
            "list flat builtin finite "
            + flat_builtin_operation
            + " at "
            + str(flat_builtin_break_at),
            lambda break_at=flat_builtin_break_at,
            operation=flat_builtin_operation: finite_flat_builtin_callback_cycle(
                break_at, operation
            ),
        )


class NextForeverItem:
    __eq__ = next
    calls = 0

    def __next__(self):
        type(self).calls += 1
        return True


def flat_builtin_progress_exhaustion():
    left = []
    right = []
    left.extend((NextForeverItem(), left))
    right.extend((NextForeverItem(), right))
    try:
        return left == right
    except Exception as exc:
        return type(exc).__name__, str(exc), NextForeverItem.calls


print("list flat builtin progress exhaustion", flat_builtin_progress_exhaustion())
outcome(
    "list flat builtin cleanup",
    lambda: finite_flat_builtin_callback_cycle(3, "eq"),
)


# A callback that returns normally still advances comparison progress. With no
# finite edge replacement, CPython keeps invoking it until native comparison
# headroom is exhausted, then raises the generic callback-entry error.
class EqualForever:
    calls = 0

    def __eq__(self, other):
        type(self).calls += 1
        return True


def callback_progress_exhaustion():
    left = []
    right = []
    left.extend((EqualForever(), left))
    right.extend((EqualForever(), right))
    try:
        return left == right
    except Exception as exc:
        return type(exc).__name__, str(exc), EqualForever.calls


print("list callback progress exhaustion", callback_progress_exhaustion())


# Callback-only recursion is governed by the ordinary Python call limit, so it
# remains catchable but uses the generic RecursionError message.
class ReenterForever:
    def __eq__(self, other):
        return forever_left == forever_right


forever_left = [ReenterForever()]
forever_right = [ReenterForever()]
outcome("list callback recursion", lambda: forever_left == forever_right)
