# Parity fixture for the native `graphlib` module (issue #3462).
#
# Exercises TopologicalSorter / CycleError against CPython 3.12.3
# `Lib/graphlib.py`: construction, `add()` bookkeeping, the
# prepare()/get_ready()/done() lifecycle, every ValueError, cycle detection
# (exact `CycleError.args`), lazy `static_order()`, hashable-node semantics
# (custom `__hash__` / `__eq__`, unhashable nodes), subclassing and
# `TopologicalSorter[int]`.  Predecessor collections are lists/tuples so the
# output never depends on set iteration order.

import graphlib
from graphlib import CycleError, TopologicalSorter
import types


def err(fn):
    try:
        fn()
    except Exception as exc:
        return (type(exc).__name__, exc.args)
    return ("OK", None)


def _prepared():
    t = TopologicalSorter()
    t.prepare()
    return t


def drain(ts):
    """Run the full prepare/get_ready/done protocol; return the groups."""
    ts.prepare()
    groups = []
    while ts.is_active():
        group = ts.get_ready()
        groups.append(group)
        ts.done(*group)
    return groups


# ── 1. Module metadata ───────────────────────────────────────────────────────

print("all:", graphlib.__all__)
print("CycleError mro:", [c.__name__ for c in CycleError.__mro__])
print("CycleError subclass of ValueError:", issubclass(CycleError, ValueError))
print("CycleError module:", CycleError.__module__, TopologicalSorter.__module__)
print("names:", CycleError.__name__, TopologicalSorter.__name__)
print("sentinels:", graphlib._NODE_OUT, graphlib._NODE_DONE)
print("GenericAlias:", graphlib.GenericAlias is types.GenericAlias)
print("_NodeInfo slots:", graphlib._NodeInfo.__slots__)
print("public:", sorted(n for n in dir(graphlib) if not n.startswith("_")))

# ── 2. GenericAlias / class_getitem ──────────────────────────────────────────

alias = TopologicalSorter[int]
print(
    "alias:",
    type(alias) is types.GenericAlias,
    alias.__origin__ is TopologicalSorter,
    alias.__args__,
    repr(alias),
)
print("alias tuple:", TopologicalSorter[int, str].__args__)

# ── 3. Fresh instance state ──────────────────────────────────────────────────

ts = TopologicalSorter()
print("vars:", vars(ts))
print("state:", ts._node2info, ts._ready_nodes, ts._npassedout, ts._nfinished)

# ── 4. Empty graph / single node ─────────────────────────────────────────────

print("empty static_order:", list(TopologicalSorter().static_order()))
e = TopologicalSorter()
e.prepare()
print("empty after prepare:", e.is_active(), bool(e), e.get_ready(), e.is_active())
print("empty graph arg:", list(TopologicalSorter({}).static_order()))

s = TopologicalSorter()
s.add("only")
print("single:", list(s.static_order()))
print("single groups:", drain(TopologicalSorter({"x": []})))

# ── 5. Linear chain, diamond, disconnected ───────────────────────────────────

chain = TopologicalSorter({3: [2], 2: [1], 1: []})
print("chain:", list(chain.static_order()))

diamond = TopologicalSorter()
diamond.add("D", "B", "C")
diamond.add("B", "A")
diamond.add("C", "A")
print("diamond groups:", drain(diamond))

diamond2 = TopologicalSorter({"D": ["B", "C"], "B": ["A"], "C": ["A"]})
print("diamond static_order:", list(diamond2.static_order()))

disc = TopologicalSorter()
disc.add("b1", "a1")
disc.add("b2", "a2")
disc.add("c", "b1", "b2")
disc.add("lonely")
print("disconnected groups:", drain(disc))

wide = TopologicalSorter({i: [0] for i in range(1, 6)})
print("wide groups:", drain(wide))

# ── 6. add() semantics: unions, duplicates, implicit nodes ───────────────────

dup = TopologicalSorter()
dup.add("c", "a", "a")
dup.add("c", "b")
dup.add("c", "b")
print("dup node2info:", [(k, v.npredecessors, v.successors) for k, v in dup._node2info.items()])
print("dup groups:", drain(dup))

union = TopologicalSorter()
union.add("x", "a")
union.add("x", "b")
union.add("y", "x")
print("union:", list(union.static_order()))

implicit = TopologicalSorter()
implicit.add("n", "p", "q")
print("implicit keys:", list(implicit._node2info))
print("add returns:", implicit.add("z"))
print("add node kw:", TopologicalSorter().add(node="k"))
print("add no preds count:", [
    (k, v.npredecessors) for k, v in implicit._node2info.items()
])

# Constructor behaves like repeated add() calls (in dict order).
built = TopologicalSorter({"a": ["b", "c"], "b": ["c"], "c": []})
manual = TopologicalSorter()
manual.add("a", "b", "c")
manual.add("b", "c")
manual.add("c")
print("ctor == add:", list(built._node2info) == list(manual._node2info),
      list(built.static_order()) == list(manual.static_order()))

# Non-dict mappings and objects with .items().
class Mapping:
    def __init__(self, data):
        self.data = data

    def items(self):
        return iter(self.data)


print("custom items():", list(TopologicalSorter(Mapping([(2, (1,)), (1, ())])).static_order()))
print("graph gen items():", list(TopologicalSorter(Mapping((n, [n - 1] if n else []) for n in range(4))).static_order()))
print("graph=None:", list(TopologicalSorter(graph=None).static_order()))
print("graph kw:", list(TopologicalSorter(graph={"b": ["a"]}).static_order()))
print("graph not mapping:", err(lambda: TopologicalSorter(5)))
print("graph list:", err(lambda: TopologicalSorter([("a", ["b"])])))
print("graph pred not iterable:", err(lambda: TopologicalSorter({"a": 5})))
print("graph pred str:", list(TopologicalSorter({"a": "bc"}).static_order()))
print("graph bad item:", err(lambda: TopologicalSorter(Mapping([(1, 2, 3)]))))
print("graph short item:", err(lambda: TopologicalSorter(Mapping([(1,)]))))
print("graph unhashable key:", err(lambda: TopologicalSorter({1: [[2]]})))
print("graph tuple preds:", list(TopologicalSorter({"z": ("y", "x")}).static_order()))
print("ctor too many:", err(lambda: TopologicalSorter({}, {})))
print("ctor bad kw:", err(lambda: TopologicalSorter(g={})))

# ── 7. Incremental lifecycle ─────────────────────────────────────────────────

ts = TopologicalSorter()
ts.add(3, 2, 1)
ts.add(2, 1)
ts.add(4, 1)
ts.prepare()
print("lifecycle ready_nodes:", ts._ready_nodes)
print("lifecycle active0:", ts.is_active(), bool(ts))
g1 = ts.get_ready()
print("lifecycle g1:", g1, type(g1).__name__, ts._npassedout, ts._nfinished)
print("lifecycle again:", ts.get_ready(), ts.get_ready())
print("lifecycle active1:", ts.is_active())
ts.done(1)
print("lifecycle after done(1):", ts._ready_nodes, ts._nfinished)
g2 = ts.get_ready()
print("lifecycle g2:", g2)
print("lifecycle active2:", ts.is_active())
ts.done(2)
ts.done(4)
print("lifecycle g3:", ts.get_ready())
print("lifecycle active3:", ts.is_active())
ts.done(3)
print("lifecycle counters:", ts._npassedout, ts._nfinished)
print("lifecycle final:", ts.get_ready(), ts.is_active(), bool(ts))
print("info state:", sorted((k, v.npredecessors) for k, v in ts._node2info.items()))

# done() with several nodes in one call.
ts = TopologicalSorter({"c": ["a", "b"]})
ts.prepare()
group = ts.get_ready()
print("multi done group:", group)
print("multi done:", ts.done(*group), ts.get_ready(), ts.is_active())
print("multi done final:", ts.done(*ts.get_ready()), ts.is_active())
ts = TopologicalSorter({"c": ["a", "b"]})
ts.prepare()
ts.get_ready()
print("done() no args:", ts.done(), ts.is_active())

# Partial progress keeps is_active true until everything is done.
ts = TopologicalSorter({"b": ["a"]})
ts.prepare()
print("partial:", ts.is_active(), ts.get_ready(), ts.is_active(), ts.get_ready(), ts.is_active())
ts.done("a")
print("partial 2:", ts.is_active(), ts.get_ready(), ts.is_active())
ts.done("b")
print("partial 3:", ts.is_active(), ts.get_ready())

# ── 8. Misuse errors ─────────────────────────────────────────────────────────

t = TopologicalSorter({"b": ["a"]})
print("get_ready before prepare:", err(t.get_ready))
print("is_active before prepare:", err(t.is_active))
print("bool before prepare:", err(lambda: bool(t)))
print("done before prepare:", err(lambda: t.done("a")))
print("done() before prepare:", err(t.done))
t.prepare()
print("prepare twice:", err(t.prepare))
print("add after prepare:", err(lambda: t.add("c")))
print("add after prepare (no preds msg):", err(lambda: t.add("c", "a")))
print("done unknown:", err(lambda: t.done("zzz")))
print("done not ready:", err(lambda: t.done("b")))
print("done not ready (still queued):", err(lambda: t.done("a")))
g = t.get_ready()
print("done not ready after get_ready:", err(lambda: t.done("b")))
t.done("a")
print("done twice:", err(lambda: t.done("a")))
print("done mixed list stops at first bad:", err(lambda: t.done("b", "b")))
print("prepare after start:", err(t.prepare))
print("state after errors:", t._npassedout, t._nfinished, t._ready_nodes)

# done(): nodes processed before the failing one are kept.
t = TopologicalSorter({"b": ["a"], "d": ["c"]})
t.prepare()
t.get_ready()
print("partial done error:", err(lambda: t.done("a", "nope", "c")))
print("partial done state:", t._nfinished, sorted(t._ready_nodes))

# add() misuse of arguments.
print("add no args:", err(lambda: TopologicalSorter().add()))
print("add bad kw:", err(lambda: TopologicalSorter().add("a", other=1)))
print("add dup kw:", err(lambda: TopologicalSorter().add("a", node="b")))
print("done bad kw:", err(lambda: _prepared().done(node=1)))
print("prepare args:", err(lambda: TopologicalSorter().prepare(1)))
print("get_ready args:", err(lambda: _prepared().get_ready(1)))
print("is_active args:", err(lambda: _prepared().is_active(1)))
print("static_order args:", err(lambda: TopologicalSorter().static_order(1)))


# ── 9. Cycles ────────────────────────────────────────────────────────────────

def cycle_of(graph):
    ts = TopologicalSorter(graph)
    try:
        ts.prepare()
    except CycleError as exc:
        return exc
    return None


exc = cycle_of({1: [1]})
print("self loop:", exc.args, type(exc).__name__, str(exc))
exc = cycle_of({1: [2], 2: [1]})
print("2-cycle:", exc.args)
exc = cycle_of({1: [3], 2: [1], 3: [2]})
print("3-cycle:", exc.args)
exc = cycle_of({i: [i + 1] for i in range(1, 8)} | {8: [1]})
print("long cycle:", exc.args)
exc = cycle_of({"x": ["a"], "a": ["b"], "b": ["c"], "c": ["a"]})
print("tail into cycle:", exc.args)
exc = cycle_of({"a": ["b"], "b": []} | {"c": ["d"], "d": ["c"]})
print("second component:", exc.args)
print("acyclic:", cycle_of({"a": ["b"], "b": []}))
exc = cycle_of({"a": ["b", "c"], "b": ["c"], "c": ["a"]})
print("multi path cycle:", exc.args)
exc = cycle_of({"p": ["q", "r"], "q": ["r"], "r": ["s"], "s": ["q"]})
print("cycle behind diamond:", exc.args)
print("cycle is list:", type(exc.args[1]).__name__, exc.args[1][0] == exc.args[1][-1])
print("caught as ValueError:", err(lambda: TopologicalSorter({1: [2], 2: [1]}).prepare()))
print("CycleError plain:", CycleError("a", [1, 2]).args, repr(CycleError("only")))

# prepare() leaves the sorter usable after a CycleError.
ts = TopologicalSorter({"free": [], "a": ["b"], "b": ["a"], "after": ["free"]})
try:
    ts.prepare()
except CycleError as e:
    print("cycle msg:", e.args)
print("after cycle ready:", ts._ready_nodes, ts.is_active())
print("after cycle get_ready:", ts.get_ready())
ts.done("free")
print("after cycle next:", ts.get_ready(), ts.is_active())
ts.done("after")
print("after cycle blocked:", ts.get_ready(), ts.is_active(), ts._nfinished, ts._npassedout)
print("prepare again after cycle:", err(ts.prepare))
print("add after cycle:", err(lambda: ts.add("late")))

# ── 10. static_order() laziness ──────────────────────────────────────────────

ts = TopologicalSorter({"b": ["a"], "c": ["b"]})
it = ts.static_order()
print("lazy before next:", ts._ready_nodes, ts._node2info.keys() == {"a", "b", "c"})
print("lazy is iterator:", iter(it) is it, hasattr(it, "__next__"))
print("first:", next(it), ts._ready_nodes, ts._npassedout, ts._nfinished)
print("second:", next(it), ts._npassedout, ts._nfinished)
print("third:", next(it), ts._npassedout, ts._nfinished)
print("exhaust:", list(it), ts._npassedout, ts._nfinished, ts.is_active())
print("exhausted next:", err(lambda: next(it)))

# Partial consumption leaves the sorter mid-flight and usable manually.
ts = TopologicalSorter({"b": ["a"], "c": ["a"], "d": ["b", "c"]})
it = ts.static_order()
print("partial first:", next(it), ts._npassedout, ts._nfinished, ts.is_active())
print("partial add:", err(lambda: ts.add("late")))
print("partial prepare:", err(ts.prepare))
print("partial next:", next(it), ts._npassedout, ts._nfinished)
print("partial rest:", list(it))

# Calling static_order() twice on one sorter prepares only once.
ts = TopologicalSorter({"b": ["a"]})
first = ts.static_order()
second = ts.static_order()
print("two iterators:", list(first), err(lambda: list(second)))

# static_order() cycle error surfaces from the first next().
ts = TopologicalSorter({1: [2], 2: [1]})
it = ts.static_order()
print("cycle lazy:", ts._ready_nodes)
print("cycle next:", err(lambda: next(it)))
print("cycle after:", err(lambda: next(it)))
print("cycle list:", err(lambda: list(TopologicalSorter({1: [2], 2: [1]}).static_order())))

# A cycle still yields the nodes that are not blocked by it? (it does not:
# prepare() raises before anything is passed out)
ts = TopologicalSorter({"x": [], "a": ["b"], "b": ["a"]})
print("cycle yields nothing:", err(lambda: list(ts.static_order())), ts._npassedout)

# Re-run after prepare() consumed the sorter.
ts = TopologicalSorter({"b": ["a"]})
ts.prepare()
print("static_order after prepare:", err(lambda: list(ts.static_order())))

# done() runs when the consumer asks for the item after the group's last one.
ts = TopologicalSorter({"c": ["a", "b"]})
it = ts.static_order()
print("group a:", next(it), ts._nfinished)
print("group b:", next(it), ts._nfinished)
print("group c:", next(it), ts._nfinished)
print("end:", err(lambda: next(it)), ts._nfinished)

# ── 11. Hashable nodes ───────────────────────────────────────────────────────

class Key:
    log = []

    def __init__(self, name, tag=0):
        self.name = name
        self.tag = tag

    def __hash__(self):
        return hash(self.name)

    def __eq__(self, other):
        return isinstance(other, Key) and self.name == other.name

    def __repr__(self):
        return f"Key({self.name!r}, {self.tag})"


ts = TopologicalSorter()
a1 = Key("a", 1)
a2 = Key("a", 2)  # equal to a1
b = Key("b")
ts.add(b, a1)
ts.add(a2)
ts.add(Key("c"), a2, b)
print("custom keys:", len(ts._node2info), list(ts._node2info))
print("custom static:", list(ts.static_order()))

ts = TopologicalSorter()
ts.add(Key("x", 1), Key("y", 1))
ts.add(Key("y", 2), Key("x", 2))
exc = None
try:
    ts.prepare()
except CycleError as e:
    exc = e
print("custom cycle:", exc.args)

ts = TopologicalSorter()
ts.add(Key("m", 1), Key("n", 1))
ts.add(Key("n", 2))
ts.prepare()
group = ts.get_ready()
print("custom get_ready:", group)
ts.done(Key("n", 99))
print("custom done by equal:", ts.get_ready(), ts.is_active())

# Mixed node types and numeric equality (1 == 1.0 == True).
ts = TopologicalSorter()
ts.add("s", 1, 2.5, (3, 4), None, frozenset({5}))
ts.add(1.0, 7)
print("mixed keys:", list(ts._node2info))
print("mixed static:", list(ts.static_order()))
print("int/float/bool merge:", len(TopologicalSorter({1: [], 1.0: [], True: []})._node2info))

# Unhashable nodes.
print("unhashable node:", err(lambda: TopologicalSorter().add([1])))
print("unhashable pred:", err(lambda: TopologicalSorter().add("a", {})))
print("unhashable done:", err(lambda: _prepared().done([1])))
t = TopologicalSorter()
try:
    t.add("ok", "p", [1])
except TypeError as exc:
    print("partial add error:", exc)
print("partial add state:", [(k, v.npredecessors, v.successors) for k, v in t._node2info.items()])


class NoHash:
    __hash__ = None


print("explicit unhashable:", err(lambda: TopologicalSorter().add(NoHash())))


class BadEq:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        raise RuntimeError("eq called")


t = TopologicalSorter()
t.add(BadEq())
print("eq raises:", err(lambda: t.add(BadEq())))

# ── 12. Subclassing and overrides ────────────────────────────────────────────

class Traced(TopologicalSorter):
    def __init__(self, graph=None):
        self.calls = []
        super().__init__(graph)

    def add(self, node, *predecessors):
        self.calls.append(("add", node, predecessors))
        return super().add(node, *predecessors)

    def prepare(self):
        self.calls.append(("prepare",))
        return super().prepare()

    def get_ready(self):
        self.calls.append(("get_ready",))
        return super().get_ready()

    def is_active(self):
        self.calls.append(("is_active",))
        return super().is_active()

    def done(self, *nodes):
        self.calls.append(("done", nodes))
        return super().done(*nodes)

    def _get_nodeinfo(self, node):
        self.calls.append(("info", node))
        return super()._get_nodeinfo(node)


t = Traced({"b": ["a"]})
print("subclass init:", t.calls)
t.calls.clear()
print("subclass static:", list(t.static_order()))
print("subclass calls:", t.calls)
print("subclass types:", type(t).__name__, isinstance(t, TopologicalSorter), list(vars(t)))
print("subclass alias:", Traced[int].__origin__ is Traced)

t = Traced({"b": ["a"]})
t.calls.clear()
t.prepare()
print("subclass bool:", bool(t))
print("subclass bool calls:", t.calls)


class Plain(TopologicalSorter):
    pass


p = Plain({"b": ["a"]})
print("plain subclass:", list(p.static_order()), type(p).__mro__[1].__name__)


class Custom(TopologicalSorter):
    def is_active(self):
        return False


c = Custom({"b": ["a"]})
print("override is_active static_order:", list(c.static_order()))
print("override bool:", bool(c))


class Boom(TopologicalSorter):
    def done(self, *nodes):
        raise KeyError("boom")


print("done override error:", err(lambda: list(Boom({"a": ["b"]}).static_order())))


class CycleSub(CycleError):
    pass


print("CycleError subclass:", CycleSub("m", [1]).args, issubclass(CycleSub, ValueError))

# Calling methods on the wrong receiver.
print("add wrong self:", err(lambda: TopologicalSorter.add(object(), 1)))
print("prepare wrong self:", err(lambda: TopologicalSorter.prepare(object())))
print("unbound no args:", err(lambda: TopologicalSorter.prepare()))
print("add unbound no args:", err(lambda: TopologicalSorter.add()))

# Attribute tampering is visible, like in CPython.
ts = TopologicalSorter({"b": ["a"]})
ts.prepare()
ts._ready_nodes.append("extra")
print("tampered ready:", err(ts.get_ready), ts._npassedout, ts._ready_nodes)
print("tampered done:", err(lambda: ts.done("extra")))

# ── 13. Bigger graph / determinism ───────────────────────────────────────────

graph = {}
for i in range(1, 40):
    graph[i] = [j for j in (i // 2, i // 3) if j]
ts = TopologicalSorter(graph)
order = list(ts.static_order())
pos = {n: i for i, n in enumerate(order)}
print("big order:", order)
print("big valid:", all(pos[p] < pos[n] for n, ps in graph.items() for p in ps))
print("big groups:", drain(TopologicalSorter(graph)))

# A graph built from a generator of pairs is consumed once.
ts = TopologicalSorter()
for n in range(5):
    ts.add(n, *range(n))
print("triangular:", list(ts.static_order()))
print("triangular counts:", [(k, v.npredecessors) for k, v in ts._node2info.items()])

# Mutating the graph argument afterwards has no effect.
g = {"b": ["a"]}
ts = TopologicalSorter(g)
g["c"] = ["b"]
g["b"].append("z")
print("graph copied:", list(ts.static_order()))

# bool() and the module truthiness rules.
ts = TopologicalSorter()
ts.prepare()
print("bool empty:", bool(ts), ts.is_active())
ts = TopologicalSorter({"a": []})
ts.prepare()
print("bool ready:", bool(ts))
ts.get_ready()
print("bool passed out:", bool(ts))
ts.done("a")
print("bool done:", bool(ts))
