# Parity test for `gc` stdlib module (CPython 3.12, issue #3246).
import gc


class Node:
    def __init__(self, name, child=None):
        self.name = name
        self.child = child


# 1. Constants
print("DEBUG_STATS:", gc.DEBUG_STATS)
print("DEBUG_COLLECTABLE:", gc.DEBUG_COLLECTABLE)
print("DEBUG_UNCOLLECTABLE:", gc.DEBUG_UNCOLLECTABLE)
print("DEBUG_SAVEALL:", gc.DEBUG_SAVEALL)
print("DEBUG_LEAK:", gc.DEBUG_LEAK)
print("DEBUG_LEAK_EQ:", gc.DEBUG_LEAK == (gc.DEBUG_COLLECTABLE | gc.DEBUG_UNCOLLECTABLE | gc.DEBUG_SAVEALL))

# 2. garbage & callbacks attributes
print("garbage_is_list:", isinstance(gc.garbage, list))
print("callbacks_is_list:", isinstance(gc.callbacks, list))

# 3. enable / disable / isenabled
was_enabled = gc.isenabled()
print("initial_enabled:", was_enabled)
gc.disable()
print("after_disable:", gc.isenabled())
gc.enable()
print("after_enable:", gc.isenabled())

# 4. get_threshold / set_threshold / get_count
orig_thresh = gc.get_threshold()
print("threshold_shape:", isinstance(orig_thresh, tuple), len(orig_thresh), all(isinstance(x, int) for x in orig_thresh))
gc.set_threshold(500)
t1 = gc.get_threshold()
print("set_threshold_1:", t1[0] == 500, t1[1] == orig_thresh[1], t1[2] == orig_thresh[2])
gc.set_threshold(600, 15)
t2 = gc.get_threshold()
print("set_threshold_2:", t2[0] == 600, t2[1] == 15, t2[2] == orig_thresh[2])
gc.set_threshold(700, 10, 10)
t3 = gc.get_threshold()
print("set_threshold_3:", t3[0] == 700, t3[1] == 10, isinstance(t3[2], int))
gc.set_threshold(*orig_thresh)

counts = gc.get_count()
print("counts_shape:", isinstance(counts, tuple), len(counts), all(isinstance(x, int) for x in counts))

# 5. get_debug / set_debug
orig_debug = gc.get_debug()
print("initial_debug:", orig_debug)
gc.set_debug(gc.DEBUG_SAVEALL)
print("after_set_debug:", gc.get_debug() == gc.DEBUG_SAVEALL)
gc.set_debug(orig_debug)
print("restored_debug:", gc.get_debug() == orig_debug)

# 6. collect() + callbacks + get_stats()
events = []


def on_gc(phase, info):
    events.append((phase, info["generation"], set(info.keys())))


gc.callbacks.append(on_gc)
try:
    before_stats = [dict(s) for s in gc.get_stats()]
    collected0 = gc.collect(0)
    collected1 = gc.collect(1)
    collected2 = gc.collect()
    after_stats = gc.get_stats()
finally:
    gc.callbacks.remove(on_gc)

print("collect_ints:", isinstance(collected0, int), isinstance(collected1, int), isinstance(collected2, int))
print("callback_phases:", [(p, g) for p, g, _ in events])
print("callback_keys_ok:", all(ks == {"generation", "collected", "uncollectable"} for _, _, ks in events))
print("stats_len:", len(after_stats))
print(
    "stats_keys_ok:",
    all(set(s.keys()) == {"collections", "collected", "uncollectable"} for s in after_stats),
)
print(
    "stats_incremented:",
    after_stats[0]["collections"] > before_stats[0]["collections"],
    after_stats[2]["collections"] > before_stats[2]["collections"],
)

# Invalid generation raises ValueError
for bad_gen in (-1, 3, 99):
    try:
        gc.collect(bad_gen)
        print("bad_collect_no_err:", bad_gen)
    except ValueError:
        print("bad_collect_valerr:", bad_gen)

# 7. is_tracked & is_finalized
print("tracked_primitives:", gc.is_tracked(None), gc.is_tracked(42), gc.is_tracked("hi"), gc.is_tracked(b"hi"))
print("tracked_empty_tuple:", gc.is_tracked(()))
print("tracked_containers:", gc.is_tracked([]), gc.is_tracked([1]), gc.is_tracked({"a": []}), gc.is_tracked(set()))
n = Node("a")
print("tracked_instance:", gc.is_tracked(n))
print("tracked_plain_object:", gc.is_tracked(object()))
print("finalized_live:", gc.is_finalized(n), gc.is_finalized([1, 2]))

# 8. get_objects / get_referents / get_referrers
sentinel_list = [Node("inner"), "marker_xyz"]
objs = gc.get_objects()
print("get_objects_list:", isinstance(objs, list), len(objs) > 0)
print("sentinel_in_objects:", any(o is sentinel_list for o in objs))
objs_g0 = gc.get_objects(0)
print("get_objects_g0_list:", isinstance(objs_g0, list))
try:
    gc.get_objects(5)
    print("bad_get_objects_no_err")
except ValueError:
    print("bad_get_objects_valerr: True")

refs = gc.get_referents(sentinel_list)
print("referents_has_str:", "marker_xyz" in refs)
print("referents_has_node:", any(r is sentinel_list[0] for r in refs))
print("referents_int:", gc.get_referents(123))

referrers = gc.get_referrers(sentinel_list[0])
print("referrers_has_list:", any(r is sentinel_list for r in referrers))

# 9. freeze / unfreeze / get_freeze_count
print("freeze_initial:", gc.get_freeze_count())
gc.freeze()
print("freeze_after:", gc.get_freeze_count() > 0)
gc.unfreeze()
print("freeze_unfrozen:", gc.get_freeze_count())
