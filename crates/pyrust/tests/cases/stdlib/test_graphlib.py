# Parity fixture for `graphlib` (issue #3208).

import graphlib
from graphlib import CycleError, TopologicalSorter
import types


def catch_error(fn):
    try:
        fn()
    except Exception as exc:
        return (type(exc).__name__, str(exc))
    return ("OK", None)


# ── 1. Module metadata & GenericAlias ─────────────────────────────────────────

print("all:", graphlib.__all__)
print("CycleError subclass:", issubclass(CycleError, ValueError))
alias = TopologicalSorter[str]
print(
    "generic alias:",
    type(alias) is types.GenericAlias,
    alias.__origin__ is TopologicalSorter,
    alias.__args__,
)

# ── 2. Empty graph ────────────────────────────────────────────────────────────

ts_empty = TopologicalSorter()
print("empty static_order:", list(ts_empty.static_order()))
print("empty is_active after:", ts_empty.is_active(), bool(ts_empty))

# ── 3. Basic DAG via constructor & static_order ───────────────────────────────

graph = {"D": {"B", "C"}, "C": {"A"}, "B": {"A"}}
ts = TopologicalSorter(graph)
order = list(ts.static_order())
assert set(order) == {"A", "B", "C", "D"}
assert order[0] == "A"
assert order[-1] == "D"
print("dag static_order valid:", len(order), order[0], order[-1])

# Deterministic order with insertion-ordered predecessors
ts_det = TopologicalSorter()
ts_det.add("D", "B", "C")
ts_det.add("B", "A")
ts_det.add("C", "A")
print("deterministic static_order:", list(ts_det.static_order()))

# ── 4. Incremental prepare / is_active / get_ready / done ─────────────────────

ts_inc = TopologicalSorter()
ts_inc.add(4, 2, 3)
ts_inc.add(2, 1)
ts_inc.add(3, 1)
ts_inc.add(1)
# Duplicate predecessors and repeated add() calls union dependencies cleanly
ts_inc.add(4, 2, 2)

ts_inc.prepare()
waves = []
while ts_inc.is_active():
    assert bool(ts_inc) is True
    ready = ts_inc.get_ready()
    waves.append(ready)
    # Calling get_ready again before done() returns empty tuple while still active
    assert ts_inc.get_ready() == ()
    assert ts_inc.is_active() is True
    ts_inc.done(*ready)
assert ts_inc.is_active() is False
assert bool(ts_inc) is False
assert ts_inc.get_ready() == ()
print("incremental waves:", waves)

# Partial done: only mark one node of a wave done at a time
ts_partial = TopologicalSorter()
ts_partial.add("end", "left", "right")
ts_partial.add("left", "root")
ts_partial.add("right", "root")
ts_partial.prepare()
assert ts_partial.get_ready() == ("root",)
ts_partial.done("root")
r1 = ts_partial.get_ready()
assert r1 == ("left", "right")
ts_partial.done("left")
assert ts_partial.get_ready() == ()
assert ts_partial.is_active() is True
ts_partial.done("right")
assert ts_partial.get_ready() == ("end",)
ts_partial.done("end")
assert ts_partial.is_active() is False
print("partial done: ok")

# Disconnected components & isolated nodes
ts_disc = TopologicalSorter()
ts_disc.add("x")
ts_disc.add("y")
ts_disc.add("z", "w")
print("disconnected:", list(ts_disc.static_order()))

# ── 5. Cycle detection & CycleError.args[1] ───────────────────────────────────

# Self-cycle
ts_self = TopologicalSorter()
ts_self.add("self", "self")
try:
    ts_self.prepare()
    assert False, "expected CycleError"
except CycleError as exc:
    print("self cycle:", exc.args[0], exc.args[1])

# 3-node cycle
ts_cyc = TopologicalSorter()
ts_cyc.add("a", "b")
ts_cyc.add("b", "c")
ts_cyc.add("c", "a")
try:
    ts_cyc.prepare()
    assert False, "expected CycleError"
except CycleError as exc:
    cycle = exc.args[1]
    assert cycle[0] == cycle[-1]
    assert len(cycle) == 4
    # In graphlib, each node in cycle is an immediate predecessor of the next
    edges = {("b", "a"), ("c", "b"), ("a", "c")}
    for i in range(len(cycle) - 1):
        assert (cycle[i], cycle[i + 1]) in edges
    print("3-cycle valid:", exc.args[0], cycle)

# Acyclic prefix can still be drained after catching CycleError
ts_mixed = TopologicalSorter()
ts_mixed.add("root")
ts_mixed.add("mid", "root")
ts_mixed.add("c1", "c2", "mid")
ts_mixed.add("c2", "c1")
try:
    ts_mixed.prepare()
    assert False, "expected CycleError"
except CycleError as exc:
    assert exc.args[1][0] == exc.args[1][-1]
drained = []
while ts_mixed.is_active():
    ready = ts_mixed.get_ready()
    drained.append(ready)
    ts_mixed.done(*ready)
print("drained after cycle:", drained)

# static_order raises CycleError when iterated
ts_cyc_static = TopologicalSorter({"x": ["y"], "y": ["x"]})
print(
    "static_order cycle:",
    catch_error(lambda: list(ts_cyc_static.static_order())),
)

# ── 6. State transition & ValueError checks ───────────────────────────────────

ts_err = TopologicalSorter()
ts_err.add("b", "a")
print("get_ready before prepare:", catch_error(lambda: ts_err.get_ready()))
print("is_active before prepare:", catch_error(lambda: ts_err.is_active()))
print("bool before prepare:", catch_error(lambda: bool(ts_err)))
print("done before prepare:", catch_error(lambda: ts_err.done("a")))

ts_err.prepare()
print("add after prepare:", catch_error(lambda: ts_err.add("c")))
print("prepare twice:", catch_error(lambda: ts_err.prepare()))
print("done unknown node:", catch_error(lambda: ts_err.done("missing")))
print("done not-yet-ready node:", catch_error(lambda: ts_err.done("b")))
print("done ready-but-not-passed-out:", catch_error(lambda: ts_err.done("a")))

assert ts_err.get_ready() == ("a",)
ts_err.done("a")
print("done twice:", catch_error(lambda: ts_err.done("a")))
assert ts_err.get_ready() == ("b",)
ts_err.done("b")
assert not ts_err.is_active()

# Nodes can be arbitrary hashable objects (tuples, ints, frozensets)
ts_hash = TopologicalSorter()
ts_hash.add((2, "b"), (1, "a"))
ts_hash.add((3, "c"), (2, "b"))
print("tuple nodes:", list(ts_hash.static_order()))

print("graphlib tests ok")
