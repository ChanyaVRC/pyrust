# Issue #3003: move_to_end must relink the entry found by dict lookup.
#
# Deleting the entry and reinserting the argument key is observably wrong when
# the argument compares equal to a different object already stored in the
# mapping.  CPython keeps the stored key object and moves only its order node.
# The native operation must also bypass Python overrides such as keys().

from collections import OrderedDict


def snapshot(mapping, stored):
    return [
        (type(key).__name__, repr(key), key is stored)
        for key in OrderedDict.keys(mapping)
    ]


# bool, int, and float collapse into one dict entry when their values compare
# equal.  Pin every stored type, in both move directions.
for last in (True, False):
    for stored, probe in ((1, True), (True, 1.0), (1.0, 1)):
        if last:
            mapping = OrderedDict([(stored, "value"), ("edge", 0)])
        else:
            mapping = OrderedDict([("edge", 0), (stored, "value")])
        mapping.move_to_end(probe, last=last)
        print(
            "numeric",
            last,
            type(stored).__name__,
            type(probe).__name__,
            snapshot(mapping, stored),
            mapping[stored],
        )


class EqualKey:
    def __init__(self, name):
        self.name = name

    def __hash__(self):
        return 17

    def __eq__(self, other):
        return isinstance(other, EqualKey)

    def __repr__(self):
        return f"EqualKey({self.name})"


# Identity preservation is not merely a numeric-type special case.
for last in (True, False):
    stored = EqualKey("stored")
    probe = EqualKey("probe")
    if last:
        mapping = OrderedDict([(stored, "value"), ("edge", 0)])
    else:
        mapping = OrderedDict([("edge", 0), (stored, "value")])
    mapping.move_to_end(probe, last=last)
    print("custom", last, snapshot(mapping, stored), mapping[probe])


class KeysOverride(OrderedDict):
    def keys(self):
        self.keys_calls += 1
        return super().keys()


# The C implementation does not dispatch through a subclass's keys() method.
for last in (True, False):
    if last:
        mapping = KeysOverride((("move", 1), ("edge", 2)))
    else:
        mapping = KeysOverride((("edge", 2), ("move", 1)))
    mapping.keys_calls = 0
    mapping.move_to_end("move", last=last)
    print(
        "keys override",
        last,
        mapping.keys_calls,
        list(OrderedDict.items(mapping)),
    )


class Missing:
    def __hash__(self):
        return 23

    def __repr__(self):
        return "Missing(needle)"


# CPython converts the optional `last` argument before hashing/looking up the
# key.  A native implementation must keep that order so a user `__bool__`
# cannot leave a previously computed entry index stale.
events = []


class MissingWithEvents:
    def __hash__(self):
        events.append("hash key")
        return 29


class LastWithEvents:
    def __bool__(self):
        events.append("truth last")
        return True


try:
    OrderedDict((("a", 1),)).move_to_end(
        MissingWithEvents(), last=LastWithEvents()
    )
except KeyError:
    print("argument order", events)


# A failed lookup carries the original argument object in KeyError, in either
# direction, and leaves the order untouched.
for last in (True, False):
    missing = Missing()
    mapping = OrderedDict((("a", 1), ("b", 2)))
    try:
        mapping.move_to_end(missing, last=last)
    except KeyError as error:
        print(
            "missing",
            last,
            repr(error.args[0]),
            error.args[0] is missing,
            str(error),
            list(mapping.items()),
        )
