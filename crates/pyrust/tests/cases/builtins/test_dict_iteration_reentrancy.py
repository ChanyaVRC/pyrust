"""Re-entering a container's iteration machinery from user code must not panic.

The positional cursors and the retained mutation-state registration
(PRs #2894 / #2885) hold their bookkeeping in `RefCell`s that are borrowed
around every step.  Any user dunder reachable from a write — `__hash__`,
`__eq__`, a `defaultdict` factory, a suspended generator's frame — can re-enter
the very container being walked, so each of these shapes has to complete with
CPython's answer rather than abort on a double borrow.
"""

from collections import defaultdict


# ── `__hash__` walks the dict it is being written into ───────────────────────
class WalkingHash:
    """Hashing this key iterates `target`, from inside `target`'s own write."""

    def __init__(self, name, target):
        self.name = name
        self.target = target
        self.seen = None

    def __hash__(self):
        self.seen = len(list(self.target))
        return hash(self.name)

    def __eq__(self, other):
        return isinstance(other, WalkingHash) and self.name == other.name

    def __repr__(self):
        return "WalkingHash(%s)" % self.name


mapping = {"a": 1, "b": 2, "c": 3}
key = WalkingHash("z", mapping)
mapping[key] = 4
print("hash write", key.seen, len(mapping), mapping[key])

lookup = WalkingHash("z", mapping)
print("hash lookup", lookup in mapping, lookup.seen, mapping[lookup])

del mapping[lookup]
print("hash delete", lookup.seen, sorted(mapping, key=repr))


# ── `__eq__` walks the dict while resolving a collision ──────────────────────
class WalkingEq:
    """All instances collide, so `__eq__` runs on every probe."""

    def __init__(self, name, target, walk_items=False):
        self.name = name
        self.target = target
        self.walk_items = walk_items
        self.calls = 0

    def __hash__(self):
        return 7

    def __eq__(self, other):
        if not isinstance(other, WalkingEq):
            return NotImplemented
        self.calls += 1
        list(self.target)
        if self.walk_items:
            list(self.target.items())
        return self.name == other.name

    def __repr__(self):
        return "WalkingEq(%s)" % self.name


colliding = {}
first = WalkingEq("first", colliding, walk_items=True)
second = WalkingEq("second", colliding, walk_items=True)
colliding[first] = 1
colliding[second] = 2
probe = WalkingEq("first", colliding, walk_items=True)
print("eq collision", len(colliding), colliding[probe], first.calls > 0)
print("eq order", [entry.name for entry in colliding])


# ── The same shapes on a set ─────────────────────────────────────────────────
members = set()
set_key = WalkingEq("only", members)
members.add(set_key)
members.add(WalkingEq("other", members))
print("set eq", len(members), WalkingEq("only", members) in members)


# ── A defaultdict factory that iterates its own mapping ──────────────────────
counted = defaultdict()


def factory():
    return len(list(counted)) * 10


counted.default_factory = factory
counted["a"] = 1
print("factory", counted["b"], counted["c"], dict(counted))

nested_default = defaultdict()


def nested_factory():
    return {key: value for key, value in nested_default.items()}


nested_default.default_factory = nested_factory
nested_default["seed"] = 1
print("nested factory", nested_default["grown"], len(nested_default))


# ── Nested loops over one container ──────────────────────────────────────────
grid = {"a": 1, "b": 2, "c": 3}
pairs = []
for outer in grid:
    for inner in grid:
        pairs.append(outer + inner)
print("nested loops", pairs)

triple = []
for one in grid:
    for two in grid.values():
        for three in grid.items():
            triple.append((one, two, three[0]))
print("triple nested", len(triple), triple[0], triple[-1])

deep = {1, 2, 3}
print("nested set", sorted((a, b) for a in deep for b in deep))

# An inner walk that finishes releases its handle while the outer walk holds
# one, so the outer walk must still refuse a mutation.
try:
    for outer in grid:
        for inner in grid:
            pass
        grid["d"] = 4
except RuntimeError as error:
    print("nested mutation", str(error))
else:
    print("nested mutation", "no error", sorted(grid))
grid.pop("d", None)


# ── A generator suspended mid-walk ───────────────────────────────────────────
def walk(container):
    for item in container:
        yield item


source = {1: "a", 2: "b", 3: "c"}
generator = walk(source)
print("suspended first", next(generator))
source[1] = "updated"
print("suspended value update", next(generator))
print("suspended rest", list(generator))

source = {1: "a", 2: "b", 3: "c"}
generator = walk(source)
print("suspended before growth", next(generator))
source[4] = "d"
try:
    print("unreachable", next(generator))
except RuntimeError as error:
    print("suspended growth", str(error))

# The generator is left suspended and simply dropped; the mapping stays usable.
source = {1: "a", 2: "b", 3: "c"}
generator = walk(source)
print("dropped", next(generator))
generator = None
source[4] = "d"
print("after drop", list(source))

# Closing a suspended generator releases its handle deterministically.
source = {1: "a", 2: "b", 3: "c"}
generator = walk(source)
print("closing", next(generator))
generator.close()
source[4] = "d"
print("after close", list(source))

# Two generators suspended on one mapping.
source = {1: "a", 2: "b", 3: "c"}
one = walk(source)
two = walk(source)
print("two generators", next(one), next(two), next(one), next(two))
print("two generators rest", list(one), list(two))


# ── An exception abandons a cursor, then the container is walked again ───────
abandoned = {"a": 1, "b": 2, "c": 3}
try:
    for entry in abandoned:
        raise ValueError(entry)
except ValueError as error:
    print("abandoned at", str(error))
print("after abandon", list(abandoned), list(abandoned.items()))
abandoned["d"] = 4
print("after abandon grow", list(abandoned))


def raising_walk(container):
    for item in container:
        if item == "b":
            raise KeyError(item)
        yield item


nested_abandon = {"a": 1, "b": 2, "c": 3}
try:
    print("raising walk", list(raising_walk(nested_abandon)))
except KeyError as error:
    print("raising walk", "KeyError", str(error))
nested_abandon["d"] = 4
print("after raising walk", list(nested_abandon))

# The same abandonment on a set, then a legal write.
abandoned_set = {"a", "b", "c"}
try:
    for entry in abandoned_set:
        raise ValueError("stop")
except ValueError:
    pass
abandoned_set.add("d")
print("set after abandon", len(abandoned_set), sorted(abandoned_set))

# Deeply stacked abandonment: three walks unwound by one exception.
stacked = {"a": 1, "b": 2}
try:
    for one in stacked:
        for two in stacked:
            for three in stacked:
                raise RuntimeError("unwind")
except RuntimeError as error:
    print("stacked abandon", str(error))
stacked["c"] = 3
print("after stacked abandon", list(stacked))
