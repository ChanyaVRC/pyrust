# Dict probes compare a stored key against a same-hash lookup key using Python
# equality even when one key is represented by a primitive PyKey variant and
# the other by PyKey::Object (issue #2820).

events = []


class EqualPrimitive:
    def __init__(self, expected, label):
        self.expected = expected
        self.label = label

    def __hash__(self):
        return hash(self.expected)

    def __eq__(self, other):
        events.append(self.label)
        return other == self.expected


def show_lookup(label, mapping, key):
    try:
        print(label, "getitem", mapping[key])
    except KeyError:
        print(label, "getitem", "KeyError")
    print(label, "get", mapping.get(key, "missing"))
    print(label, "contains", key in mapping)


# Both storage/probe directions use user equality for int, str, and
# fractional-float keys.
stored_int = EqualPrimitive(1, "stored-int")
show_lookup("user-int", {stored_int: "value"}, 1)
print("events", events)


class CrossHashStored:
    def __hash__(self):
        return 9

    def __eq__(self, other):
        events.append("cross-hash-stored")
        return isinstance(other, CrossHashQuery)


class CrossHashQuery:
    def __hash__(self):
        return 9

    def __eq__(self, other):
        events.append("cross-hash-query")
        return isinstance(other, int)


# Hashes 1 and 9 initially share a size-8 slot. Deleting hash 1 leaves a
# global DUMMY which primitive 9 reuses ahead of the older hash-9 Object.
cross_hash_stored = CrossHashStored()
cross_hash_dict = {1: "deleted", cross_hash_stored: "object"}
del cross_hash_dict[1]
cross_hash_dict[9] = "primitive"
events.clear()
print("cross-hash-dummy", cross_hash_dict[CrossHashQuery()])
print("events", events)


# Recursive tuple keys cross the same representation boundary.  Equality must
# deduplicate in both insertion directions while preserving the first tuple.
events.clear()
nested_object = EqualPrimitive(1, "nested-object-first")
nested_object_first = {(nested_object,): "a"}
nested_object_first[(1,)] = "b"
print(
    "nested-object-first",
    len(nested_object_first),
    nested_object_first[(1,)],
    next(iter(nested_object_first))[0] is nested_object,
)
print("events", events)

events.clear()
nested_probe = EqualPrimitive(1, "nested-primitive-first")
nested_primitive_first = {(1,): "a"}
nested_primitive_first[(nested_probe,)] = "b"
print(
    "nested-primitive-first",
    len(nested_primitive_first),
    nested_primitive_first[(1,)],
    next(iter(nested_primitive_first))[0] is nested_probe,
)
print("events", events)


# Frozenset keys recurse through set membership while comparing their
# elements. That membership must preserve the same Object/primitive crossing
# in both directions, including when the frozenset is nested inside a tuple.
events.clear()
frozen_object = EqualPrimitive(1, "frozen-object-first")
frozen_object_first = {frozenset((frozen_object,)): "a"}
frozen_object_first[frozenset((1,))] = "b"
print(
    "frozen-object-first",
    len(frozen_object_first),
    frozen_object_first[frozenset((1,))],
    next(iter(next(iter(frozen_object_first)))) is frozen_object,
)
print("events", events)

events.clear()
frozen_probe = EqualPrimitive(1, "frozen-primitive-first")
frozen_primitive_first = {frozenset((1,)): "a"}
frozen_primitive_first[frozenset((frozen_probe,))] = "b"
print(
    "frozen-primitive-first",
    len(frozen_primitive_first),
    frozen_primitive_first[frozenset((1,))],
    next(iter(next(iter(frozen_primitive_first)))) is frozen_probe,
)
print("events", events)

events.clear()
tuple_frozen_object = EqualPrimitive(1, "tuple-frozen-object")
tuple_frozen = {(frozenset((tuple_frozen_object,)),): "a"}
tuple_frozen[(frozenset((1,)),)] = "b"
print(
    "tuple-frozen-object",
    len(tuple_frozen),
    tuple_frozen[(frozenset((1,)),)],
)
print("events", events)


class FrozenExactCollision:
    def __init__(self):
        self.armed = False

    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        if self.armed:
            events.append("frozen-exact-collision")
            return isinstance(other, FrozenExactPeer)
        return False


class FrozenExactPeer:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        if frozen_exact_collision.armed:
            events.append("frozen-exact-peer")
        return False


# An exact primitive element in the probe frozenset must not bypass an earlier
# same-hash user element. The later peer then matches that user element, making
# the two frozensets equal while preserving the observable comparison order.
frozen_exact_collision = FrozenExactCollision()
frozen_exact_peer = FrozenExactPeer()
frozen_exact_stored = frozenset((1, frozen_exact_peer))
frozen_exact_probe = frozenset((frozen_exact_collision, 1))
frozen_exact_collision.armed = True
events.clear()
frozen_exact_dict = {frozen_exact_stored: "a"}
frozen_exact_dict[frozen_exact_probe] = "b"
print("frozen-exact", len(frozen_exact_dict), frozen_exact_dict[frozen_exact_stored])
print("events", events)


frozen_probe_order_events = []
frozen_probe_order_armed = False


class FrozenProbeOrderNeedle:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        return other == 1


class FrozenProbeOrderCollision:
    def __init__(self, label):
        self.label = label

    def __hash__(self):
        return 1

    def __eq__(self, other):
        if frozen_probe_order_armed and isinstance(other, FrozenProbeOrderNeedle):
            frozen_probe_order_events.append(self.label)
            if self.label == 1:
                raise RuntimeError("unexpected candidate")
        return self is other


# Frozenset membership walks CPython's set-table perturb chain, not insertion
# order. Multiple same-hash collisions make that distinction observable before
# the exact primitive match terminates the probe.
frozen_probe_order_shared = [FrozenProbeOrderCollision(i) for i in range(4)]
frozen_probe_order_stored = frozenset(
    frozen_probe_order_shared + [FrozenProbeOrderNeedle()]
)
frozen_probe_order_query = frozenset(frozen_probe_order_shared + [1])
frozen_probe_order_dict = {frozen_probe_order_stored: "hit"}
frozen_probe_order_armed = True
try:
    print(
        "frozen-probe-order",
        frozen_probe_order_query in frozen_probe_order_dict,
        frozen_probe_order_events,
    )
except Exception as exc:
    print(
        "frozen-probe-order",
        type(exc).__name__,
        str(exc),
        frozen_probe_order_events,
    )


frozen_source_history_events = []
frozen_source_history_armed = False


class FrozenSourceHistoryProbe:
    def __hash__(self):
        return 0

    def __eq__(self, other):
        return other == 0


class FrozenSourceHistoryCollision:
    def __hash__(self):
        return 0

    def __eq__(self, other):
        if frozen_source_history_armed and isinstance(
            other, FrozenSourceHistoryProbe
        ):
            frozen_source_history_events.append("collision")
            raise RuntimeError("cpython visits collision")
        return self is other


# frozenset(exact_set) inherits the source set's slot order. A removal leaves a
# dummy which the reinsertion reuses; rebuilding only from the final iteration
# order incorrectly lets the exact primitive match bypass this earlier user key.
frozen_source_history_collision = FrozenSourceHistoryCollision()


def frozen_from_mutated_source(last):
    source = set((frozen_source_history_collision, last))
    source.remove(frozen_source_history_collision)
    source.add(frozen_source_history_collision)
    return frozenset(source)


frozen_source_history_stored = frozen_from_mutated_source(
    FrozenSourceHistoryProbe()
)
frozen_source_history_query = frozen_from_mutated_source(0)
frozen_source_history_dict = {frozen_source_history_stored: "hit"}
frozen_source_history_armed = True
try:
    print(
        "frozen-source-history",
        frozen_source_history_query in frozen_source_history_dict,
        frozen_source_history_events,
    )
except Exception as exc:
    print(
        "frozen-source-history",
        type(exc).__name__,
        str(exc),
        frozen_source_history_events,
    )


frozen_presized_events = []
frozen_presized_armed = False


class FrozenPresizedProbe:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        return other == 1


class FrozenPresizedCollision:
    def __init__(self, label):
        self.label = label

    def __hash__(self):
        return 1

    def __eq__(self, other):
        if frozen_presized_armed and isinstance(other, FrozenPresizedProbe):
            frozen_presized_events.append(self.label)
            if self.label == 1:
                raise RuntimeError("presized candidate")
        return self is other


# CPython's exact-dict constructor path pre-sizes the destination set before
# merging keys. That produces a different observable probe chain from clean
# incremental insertion even though the final frozenset items are identical.
frozen_presized_shared = [FrozenPresizedCollision(i) for i in range(4)]
frozen_presized_stored = frozenset(
    dict.fromkeys(frozen_presized_shared + [FrozenPresizedProbe()])
)
frozen_presized_query = frozenset(dict.fromkeys(frozen_presized_shared + [1]))
frozen_presized_dict = {frozen_presized_stored: "hit"}
frozen_presized_armed = True
try:
    print(
        "frozen-presized-dict",
        frozen_presized_query in frozen_presized_dict,
        frozen_presized_events,
    )
except Exception as exc:
    print(
        "frozen-presized-dict",
        type(exc).__name__,
        str(exc),
        frozen_presized_events,
    )


def exercise_frozen_algebra_provenance(
    label,
    operation,
    source_kind,
    target,
    prefix,
    count=1,
    rotate=False,
    reverse=False,
):
    events = []
    armed = [False]

    class Probe:
        def __hash__(self):
            return hash(target)

        def __eq__(self, other):
            return other == target

    class Collision:
        def __hash__(self):
            return hash(target)

        def __eq__(self, other):
            if armed[0] and isinstance(other, Probe):
                events.append("collision")
            return self is other

    collisions = [Collision() for _ in range(count)]
    if rotate:
        collisions = collisions[1:] + collisions[:1]
    if reverse:
        collisions.reverse()

    def build(last):
        sequence = collisions + [last]
        if operation == "difference":
            destination = set(sequence + [prefix, prefix + 1_000_000])
            if source_kind == "set":
                source = {prefix}
            else:
                source = {prefix: None}
            return frozenset(destination.difference(source))
        if source_kind == "set":
            source = set(sequence)
        else:
            source = dict.fromkeys(sequence)
        if operation == "union":
            result = {prefix}.union(source)
        elif operation == "intersection":
            result = set(sequence + [prefix]).intersection(source)
        else:
            result = {prefix}.symmetric_difference(source)
        return frozenset(result)

    stored = build(Probe())
    query = build(target)
    mapping = {stored: "hit"}
    armed[0] = True
    print(label, query in mapping, events)


# Set algebra scans and mutates CPython slot tables rather than semantic
# insertion order. Exact set/dict branches also have distinct copy, pre-size,
# and traversal rules; rebuilding their results changes a later frozenset probe.
exercise_frozen_algebra_provenance(
    "frozen-union-set", "union", "set", 7, 1_000_088
)
exercise_frozen_algebra_provenance(
    "frozen-union-dict",
    "union",
    "dict",
    7,
    1_000_209,
    count=3,
    rotate=True,
)
exercise_frozen_algebra_provenance(
    "frozen-intersection-set", "intersection", "set", 7, 1_000_138
)
exercise_frozen_algebra_provenance(
    "frozen-difference-set", "difference", "set", 7, 1_000_140
)
exercise_frozen_algebra_provenance(
    "frozen-difference-dict", "difference", "dict", 7, 1_000_141
)
exercise_frozen_algebra_provenance(
    "frozen-symmetric-set",
    "symmetric",
    "set",
    -2,
    1_000_366,
    count=3,
)
exercise_frozen_algebra_provenance(
    "frozen-symmetric-dict",
    "symmetric",
    "dict",
    -2,
    1_000_367,
    count=3,
    reverse=True,
)


frozen_primitive_union_events = []
frozen_primitive_union_armed = False
frozen_primitive_union_target = 7


class FrozenPrimitiveUnionProbe:
    def __hash__(self):
        return hash(frozen_primitive_union_target)

    def __eq__(self, other):
        if frozen_primitive_union_armed:
            frozen_primitive_union_events.append("probe")
        return other == frozen_primitive_union_target


# The primitive-only union fast path must retain exact-set merge topology too:
# a user key added afterward can make that earlier table shape observable.
frozen_primitive_union_modulus = 2**61 - 1
frozen_primitive_union_items = [
    frozen_primitive_union_target + frozen_primitive_union_modulus * (index + 1)
    for index in range(4)
]


def frozen_from_primitive_union(last):
    result = {10_000_168}.union(set(frozen_primitive_union_items))
    result.add(last)
    return frozenset(result)


frozen_primitive_union_stored = frozen_from_primitive_union(
    FrozenPrimitiveUnionProbe()
)
frozen_primitive_union_query = frozen_from_primitive_union(
    frozen_primitive_union_target
)
frozen_primitive_union_dict = {frozen_primitive_union_stored: "hit"}
frozen_primitive_union_armed = True
print(
    "frozen-primitive-union",
    frozen_primitive_union_query in frozen_primitive_union_dict,
    frozen_primitive_union_events,
)


class PlainDictSubclass(dict):
    pass


# Inherited dict assignment must route through the same equality-aware backing
# mutation as a plain dict and preserve whichever key object was stored first.
subclass_object_key = EqualPrimitive(1, "subclass-object-first")
subclass_object_first = PlainDictSubclass()
subclass_object_first[subclass_object_key] = "a"
subclass_object_first[1] = "b"
print(
    "subclass-object-first",
    len(subclass_object_first),
    subclass_object_first[subclass_object_key],
    next(iter(subclass_object_first)) is subclass_object_key,
)

subclass_object_probe = EqualPrimitive(1, "subclass-primitive-first")
subclass_primitive_first = PlainDictSubclass()
subclass_primitive_first[1] = "a"
subclass_primitive_first[subclass_object_probe] = "b"
print(
    "subclass-primitive-first",
    len(subclass_primitive_first),
    subclass_primitive_first[1],
    next(iter(subclass_primitive_first)) is subclass_object_probe,
)

subclass_fromkeys_key = EqualPrimitive(1, "subclass-fromkeys")
subclass_fromkeys = PlainDictSubclass.fromkeys([subclass_fromkeys_key, 1], "value")
print(
    "subclass-fromkeys",
    len(subclass_fromkeys),
    subclass_fromkeys[1],
    next(iter(subclass_fromkeys)) is subclass_fromkeys_key,
)


class FromKeysCollision:
    def __init__(self, value):
        self.value = value

    def __hash__(self):
        return self.value

    def __eq__(self, other):
        events.append("fromkeys-layout")
        return False


# Generic fromkeys iterables grow from the shared empty table. Presizing from
# the materialized iterable length is observably wrong when most inputs are
# duplicates, for both exact dict and dict-subclass destinations.
fromkeys_layout_keys = [0] * 28
fromkeys_layout_keys.extend(
    [
        -6,
        6,
        FromKeysCollision(-9),
        FromKeysCollision(16),
        FromKeysCollision(15),
        -10,
        FromKeysCollision(-17),
        -17,
        FromKeysCollision(18),
        FromKeysCollision(-4),
        FromKeysCollision(8),
        FromKeysCollision(-20),
    ]
)
exact_fromkeys_layout = dict.fromkeys(fromkeys_layout_keys)
subclass_fromkeys_layout = PlainDictSubclass.fromkeys(fromkeys_layout_keys)
for label, mapping in [
    ("exact-fromkeys-layout", exact_fromkeys_layout),
    ("subclass-fromkeys-layout", subclass_fromkeys_layout),
]:
    events.clear()
    print(label, FromKeysCollision(-20) in mapping, len(events))

events.clear()
query_int = EqualPrimitive(1, "query-int")
show_lookup("int-user", {1: "value"}, query_int)
print("events", events)

events.clear()
stored_str = EqualPrimitive("needle", "stored-str")
show_lookup("user-str", {stored_str: "value"}, "needle")
print("events", events)

events.clear()
query_str = EqualPrimitive("needle", "query-str")
show_lookup("str-user", {"needle": "value"}, query_str)
print("events", events)

events.clear()
stored_float = EqualPrimitive(0.5, "stored-float")
show_lookup("user-float", {stored_float: "value"}, 0.5)
print("events", events)

events.clear()
query_float = EqualPrimitive(0.5, "query-float")
show_lookup("float-user", {0.5: "value"}, query_float)
print("events", events)

# A mixed dict does not duplicate Object-only Python-hash buckets in its side
# index. Distinct equal Objects at a hash unrelated to the primitive key still
# use the native Object bucket and dispatch stored-left equality.
events.clear()
mixed_native_stored = EqualPrimitive(2, "mixed-native-stored")
mixed_native_query = EqualPrimitive(2, "mixed-native-query")
show_lookup(
    "mixed-native-object",
    {1: "primitive", mixed_native_stored: "object"},
    mixed_native_query,
)
print("events", events)

# Assigning an equal cross-type key updates the value in place.  The first
# key object and its insertion position are retained in both directions.
events.clear()
original_object = EqualPrimitive(1, "object-update")
updated = {original_object: "old"}
updated[1] = "new"
print("update-object", len(updated), updated[original_object], next(iter(updated)) is original_object)
print("events", events)

events.clear()
object_update = EqualPrimitive(1, "primitive-update")
updated = {1: "old"}
updated[object_update] = "new"
print("update-primitive", len(updated), updated[1], next(iter(updated)) is object_update)
print("events", events)

# Bulk insertion shares the same deduplication rule.
events.clear()
bulk_object = EqualPrimitive(0.5, "bulk-object")
bulk_updated = {bulk_object: "old"}
bulk_updated.update({0.5: "new"})
print("bulk-update", len(bulk_updated), bulk_updated[bulk_object], next(iter(bulk_updated)) is bulk_object)
print("events", events)

# Deletion uses the same cross-type lookup in both directions.
events.clear()
deleted_object = EqualPrimitive(1, "delete-object")
to_delete = {deleted_object: "value"}
del to_delete[1]
print("delete-object", len(to_delete))
print("events", events)

events.clear()
delete_probe = EqualPrimitive(1, "delete-primitive")
to_delete = {1: "value"}
del to_delete[delete_probe]
print("delete-primitive", len(to_delete))
print("events", events)


class Collision:
    def __init__(self, label):
        self.label = label

    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append(self.label)
        return False


# A matching hash alone does not make unlike keys equal.
events.clear()
stored_collision = Collision("stored-collision")
show_lookup("collision-user", {stored_collision: "value"}, 1)
print("events", events)

events.clear()
query_collision = Collision("query-collision")
show_lookup("collision-primitive", {1: "value"}, query_collision)
print("events", events)


class StoredNotImplemented:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append("stored-not-implemented")
        return NotImplemented


class ProbeFallback:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append("probe-fallback")
        return True


# Dict comparison starts with the stored key and honors reflected fallback.
events.clear()
show_lookup("not-implemented", {StoredNotImplemented(): "value"}, ProbeFallback())
print("events", events)


class OrderedStored:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append("ordered-stored")
        return isinstance(other, OrderedProbe)


class OrderedProbe:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append("ordered-probe")
        return False


# Mixed primitive/Object candidates are compared in dictionary insertion
# order.  The primitive's reflected comparison runs before the later Object
# candidate and is observable even though that later candidate is the match.
ordered_stored = OrderedStored()
ordered = {1: "primitive", ordered_stored: "object"}
events.clear()
show_lookup("ordered-mixed", ordered, OrderedProbe())
print("events", events)

# Even an exact primitive hit must follow the shared Python-hash probe chain:
# an earlier unequal Object collision runs __eq__ before the primitive is
# reached. The same ordering applies when replacing that exact key.
exact_collision = Collision("exact-collision")
exact_collision_dict = {exact_collision: "object", 1: "primitive"}
events.clear()
show_lookup("exact-primitive", exact_collision_dict, 1)
print("events", events)
events.clear()
exact_collision_dict[1] = "updated"
print("exact-update", exact_collision_dict[1])
print("events", events)

# Identity short-circuit applies only when CPython reaches that exact Object.
# Earlier same-hash Objects still run stored-left equality first.
exact_object_first = Collision("exact-object-first")
exact_object_target = Collision("exact-object-target")
exact_object_dict = {exact_object_first: "first", exact_object_target: "target"}
events.clear()
print("exact-object", exact_object_dict[exact_object_target])
print("events", events)


class StringCollision:
    def __hash__(self):
        return hash("probe_name")

    def __eq__(self, other):
        events.append("exact-global-collision")
        return False


# Optimized string lookup must not bypass the same ordered chain when the exact
# string exists after an unequal user key with the same hash.
string_namespace = {
    StringCollision(): "object",
    "probe_name": "loaded",
}
exact_string_collision_observed = []
events.clear()
print("exact-string getitem", string_namespace["probe_name"])
exact_string_collision_observed.append(bool(events))
events.clear()
print("exact-string get", string_namespace.get("probe_name"))
exact_string_collision_observed.append(bool(events))
events.clear()
print("exact-string contains", "probe_name" in string_namespace)
exact_string_collision_observed.append(bool(events))
print("events", exact_string_collision_observed)


class Boom:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        raise RuntimeError("boom")


# Errors from the stored key's equality propagate through every lookup API.
boom_dict = {Boom(): "value"}
try:
    boom_dict[1]
except RuntimeError as exc:
    print("boom-getitem", str(exc))
try:
    boom_dict.get(1)
except RuntimeError as exc:
    print("boom-get", str(exc))
try:
    1 in boom_dict
except RuntimeError as exc:
    print("boom-contains", str(exc))


class Mutating:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append("mutating")
        mutating_dict.clear()
        return True


# User equality may mutate the dict.  A removed candidate is not returned or
# updated, and no backing-map borrow may remain live across the callback.
events.clear()
mutating_key = Mutating()
mutating_dict = {mutating_key: "stored"}
print("mutating-get", mutating_dict.get(1, "missing"), len(mutating_dict))
print("events", events)

events.clear()
mutating_dict = {mutating_key: "stored"}
mutating_dict[1] = "new"
print("mutating-set", len(mutating_dict), mutating_dict[1], next(iter(mutating_dict)) is mutating_key)
print("events", events)


class ReplacingDuringEquality:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        restart_dict.clear()
        restart_dict[1] = "replacement"
        return True


# A structural mutation during equality invalidates the old candidate table.
# Lookup restarts against the live dict and finds the newly inserted primitive
# key instead of treating the removed Object candidate as a miss.
restart_dict = {}
restart_key = ReplacingDuringEquality()
restart_dict[restart_key] = "original"
print("reentrant-replacement", restart_dict.get(1, "missing"), len(restart_dict))


class ReplacingValueDuringEquality:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        value_restart_dict[self] = "replacement"
        return True


# Rewriting only the candidate's value leaves the key table stable.  Lookup
# must not restart the comparison, but it must return the refreshed live value.
value_restart_key = ReplacingValueDuringEquality()
value_restart_dict = {value_restart_key: "original"}
print("reentrant-value", value_restart_dict.get(1, "missing"), len(value_restart_dict))


class ProbeOrderStored:
    def __init__(self, label, match=False):
        self.label = label
        self.match = match

    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append(self.label)
        return self.match and isinstance(other, ProbeOrderQuery)


class ProbeOrderQuery:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append("probe-order-query")
        return isinstance(other, int)


# Dict probes follow hash-table slot order, not surviving insertion order.  The
# final Object key reuses the deleted first slot and therefore matches before
# the primitive key gets a reflected comparison opportunity.
probe_a = ProbeOrderStored("probe-order-a")
probe_b = ProbeOrderStored("probe-order-b")
probe_c = ProbeOrderStored("probe-order-c", True)
probe_order_dict = {probe_a: "a"}
probe_order_dict[1] = "primitive"
probe_order_dict[probe_b] = "b"
del probe_order_dict[probe_a]
probe_order_dict[probe_c] = "object"
events.clear()
print("mixed-probe-order", probe_order_dict[ProbeOrderQuery()])
print("events", events)


class HomogeneousDeletedStored:
    def __init__(self, label):
        self.label = label

    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append(self.label)
        return isinstance(other, HomogeneousDeletedQuery)


class HomogeneousDeletedQuery:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append("homogeneous-deleted-query")
        return isinstance(other, int)


# A deletion slot created before the dict first becomes mixed is still part of
# its live table.  The primitive insertion reuses that slot, so its reflected
# comparison must run before the surviving Object key's comparison.
homogeneous_deleted_a = HomogeneousDeletedStored("homogeneous-deleted-a")
homogeneous_deleted_b = HomogeneousDeletedStored("homogeneous-deleted-b")
homogeneous_deleted_dict = {
    homogeneous_deleted_a: "a",
    homogeneous_deleted_b: "object",
}
del homogeneous_deleted_dict[homogeneous_deleted_a]
homogeneous_deleted_dict[1] = "primitive"
events.clear()
print(
    "homogeneous-deleted-probe-order",
    homogeneous_deleted_dict[HomogeneousDeletedQuery()],
)
print("events", events)


class LayoutStored:
    def __init__(self, label, match=False):
        self.label = label
        self.match = match

    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append(self.label)
        return self.match and isinstance(other, LayoutQuery)


class LayoutQuery:
    def __hash__(self):
        return hash(1)

    def __eq__(self, other):
        events.append("layout-query")
        return False


def make_dense_layout():
    first = LayoutStored("dense-a")
    survivor = LayoutStored("dense-b", True)
    mapping = {first: "a"}
    mapping[1] = "primitive"
    mapping[survivor] = "b"
    del mapping[first]
    return mapping


def show_layout_lookup(label, mapping):
    events.clear()
    print(label, mapping[LayoutQuery()])
    print("events", events)


# Exhausting dk_usable compacts all live entries before the new key is placed.
# A stale per-hash tombstone incorrectly lets the new Object win this lookup.
resize_a = LayoutStored("resize-a")
resize_b = LayoutStored("resize-b", True)
resize_c = LayoutStored("resize-c", True)
resize_dict = {resize_a: "a"}
resize_dict[1] = "primitive"
resize_dict[resize_b] = "b"
del resize_dict[resize_a]
resize_dict[100] = "filler-100"
resize_dict[101] = "filler-101"
resize_dict[resize_c] = "c"
show_layout_lookup("resize-compaction", resize_dict)

# A two-thirds-dense copy preserves the key table and its dummy.  The next
# Object reuses that dummy and is encountered before the surviving entries.
dense_copy = make_dense_layout().copy()
dense_copy_new = LayoutStored("dense-copy-c", True)
dense_copy[dense_copy_new] = "c"
show_layout_lookup("dense-copy", dense_copy)

# A sparse copy is rebuilt from live insertion order, dropping its dummies.
sparse_a = LayoutStored("sparse-a")
sparse_b = LayoutStored("sparse-b", True)
sparse_deleted = [LayoutStored("sparse-deleted") for _ in range(4)]
sparse_source = {sparse_a: "a"}
sparse_source[1] = "primitive"
sparse_source[sparse_b] = "b"
for sparse_key in sparse_deleted:
    sparse_source[sparse_key] = "deleted"
del sparse_source[sparse_a]
for sparse_key in sparse_deleted:
    del sparse_source[sparse_key]
sparse_copy = sparse_source.copy()
sparse_copy_new = LayoutStored("sparse-copy-c", True)
sparse_copy[sparse_copy_new] = "c"
show_layout_lookup("sparse-copy", sparse_copy)

# Dict union starts from PyDict_Copy(lhs), so it follows the same dense/sparse
# distinction instead of unconditionally rebuilding from live entries.
dense_union = make_dense_layout() | {}
dense_union_new = LayoutStored("dense-union-c", True)
dense_union[dense_union_new] = "c"
show_layout_lookup("dense-union", dense_union)

sparse_union = sparse_source | {}
sparse_union_new = LayoutStored("sparse-union-c", True)
sparse_union[sparse_union_new] = "c"
show_layout_lookup("sparse-union", sparse_union)

# popitem removes the last compact entry but leaves a DUMMY in the indices.
# Removing a temporary tail must not discard the earlier probe history.
popitem_dict = make_dense_layout()
popitem_new = LayoutStored("popitem-c", True)
popitem_dict[popitem_new] = "c"
popitem_dict[100] = "tail"
popitem_dict.popitem()
show_layout_lookup("popitem-history", popitem_dict)
