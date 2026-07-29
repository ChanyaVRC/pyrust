"""A small key walk holds its order in yielded form and converts back on demand.

Dict-key and set walks under the eager capture size keep their frozen order as
the values they yield. Keys that cannot be reproduced from that value keep the
key-order snapshot instead, and any mutation converts the frozen order back
before the general walk resumes. Both representations must iterate identically,
including through the recovery and terminal-entry paths.

Set output is aggregated because CPython orders sets by hash.
"""


def walk(label, mapping):
    print(label, list(mapping), sorted(mapping.values(), key=repr))


# Key shapes the frozen order accepts.
walk("int", {1: "a", -1: "b", 2**70: "c", -(2**70): "d"})
walk("bool", {True: "a", False: "b"})
walk("float", {1.5: "a", -0.0: "b", 1e308: "c"})
walk("str", {"a": 1, "": 2, "é": 3})
walk("bytes", {b"a": 1, b"": 2})
walk("singleton", {None: 1, ...: 2})
walk("complex", {1 + 2j: "a", 3 - 1j: "b"})

# Key shapes it declines; the key-order snapshot walks those unchanged.
walk("tuple", {(1, 2): "a", (): "b"})
walk("frozenset", {frozenset({1}): "a", frozenset(): "b"})


class Key:
    def __init__(self, name):
        self.name = name

    def __hash__(self):
        return hash(self.name)

    def __eq__(self, other):
        return isinstance(other, Key) and self.name == other.name

    def __repr__(self):
        return "Key(%s)" % self.name


walk("instance", {Key("x"): 1, Key("y"): 2})

# One ineligible key decides the representation for the whole walk.
walk("mixed", {1: "a", (2,): "b", "c": 3})

walk("empty", {})
walk("single", {"only": 1})

# `True` and `1` are the same key: the frozen order must yield the stored key.
first_wins = {1: "int"}
first_wins[True] = "bool"
walk("bool collapses onto int", first_wins)

nan = float("nan")
print("nan key", [repr(key) for key in {nan: 1, 2.0: 2}])


# A mutation converts the frozen order back and recovers exact seen history.
def recover(label, keys, removed, added):
    mapping = {key: index for index, key in enumerate(keys)}
    iterator = iter(mapping)
    prefix = [next(iterator)]
    del mapping[removed]
    mapping[added] = -1
    suffix = list(iterator)
    print(
        label,
        prefix,
        added in suffix,
        removed in suffix,
        len(prefix) + len(suffix),
        sorted(set(prefix) & set(suffix), key=repr),
    )


recover("recover int", (10, 20, 30, 40), 40, 50)
recover("recover str", ("a", "b", "c", "d"), "d", "e")
recover("recover tuple", ((1,), (2,), (3,), (4,)), (4,), (5,))
recover("recover mixed", (1, (2,), "c", 4.5), 4.5, "e")


# Growing during iteration still raises from the frozen representation.
for keys in ((1, 2, 3), ("a", "b", "c"), ((1,), (2,), (3,))):
    mapping = {key: 0 for key in keys}
    try:
        for key in mapping:
            mapping[repr(key)] = 1
    except Exception as exc:
        print("grow guard", keys[0], type(exc).__name__, str(exc))


# Deleting and reinserting the final entry is the terminal-watch path.
for keys in ((1, 2, 3), ("a", "b", "c")):
    mapping = {key: 0 for key in keys}
    seen = []
    try:
        for key in mapping:
            seen.append(key)
            if key == keys[-1]:
                del mapping[keys[-1]]
                mapping[keys[-1]] = 1
    except Exception as exc:
        print("terminal reinsert", keys[0], seen, type(exc).__name__, str(exc))
    else:
        print("terminal reinsert", keys[0], seen, "no error")


# An abandoned walk releases nothing back; the next walk must still be correct.
mapping = {"a": 1, "b": 2, "c": 3}
for key in mapping:
    break
print("after break", list(mapping), list(mapping.keys()), list(mapping.values()))


# Sets take the same frozen order.
for members in ((1, 2, 3), ("a", "b", "c"), ((1,), (2,)), (1, "b", (3,))):
    values = set(members)
    print("set walk", len(list(values)), sorted(values, key=repr))

values = {1, 2, 3, 4}
iterator = iter(values)
first = next(iterator)
values.discard(first)
values.add(99)
rest = list(iterator)
print("set recovery", 99 in rest, first in rest, len(rest) + 1)

members = {1, 2, 3}
try:
    for value in members:
        members.add(value + 10)
except Exception as exc:
    print("set guard", type(exc).__name__, str(exc))
