# copy module — built-in-backed containers must not share their backing store
# with their copies (issue #2935).
#
# `copy.copy` is a *shallow* copy: the structure is independent (writing to the
# copy is invisible to the original and vice versa) while the values inside stay
# shared.  Covers the whole family of built-in-backed containers — OrderedDict,
# deque, defaultdict, Counter, user subclasses of dict/list/set/bytearray, plain
# bytearray, and an instance `__dict__` — through copy.copy, copy.deepcopy and
# their own `.copy()` / constructors.

import copy
from collections import OrderedDict, deque, defaultdict, Counter

# ── OrderedDict ──────────────────────────────────────────────────────────────

od = OrderedDict(a=1, b=2)
shallow = copy.copy(od)
shallow["z"] = 9
assert list(od) == ["a", "b"], list(od)
assert list(shallow) == ["a", "b", "z"], list(shallow)

# …and the other direction: writes to the original stay out of the copy.
od["q"] = 3
assert list(shallow) == ["a", "b", "z"], list(shallow)

# Reordering the copy leaves the original's order alone.
ordered = OrderedDict(a=1, b=2, c=3)
reordered = copy.copy(ordered)
reordered.move_to_end("a")
assert list(ordered) == ["a", "b", "c"], list(ordered)
assert list(reordered) == ["b", "c", "a"], list(reordered)

# The already-correct paths keep working.
for clone in (
    copy.deepcopy(OrderedDict(a=1)),
    OrderedDict(a=1).copy(),
    OrderedDict(OrderedDict(a=1)),
):
    clone["z"] = 9
    assert list(clone) == ["a", "z"], list(clone)

# Shallow means the *values* are shared, deep means they are not.
shared = [1]
od = OrderedDict(x=shared)
copy.copy(od)["x"].append(2)
assert shared == [1, 2], shared
copy.deepcopy(od)["x"].append(3)
assert shared == [1, 2], shared

# ── deque ────────────────────────────────────────────────────────────────────

dq = deque([1, 2, 3], maxlen=5)
shallow = copy.copy(dq)
shallow.append(4)
assert list(dq) == [1, 2, 3], list(dq)
assert shallow.maxlen == 5, shallow.maxlen

deep = copy.deepcopy(dq)
deep.append(9)
assert list(dq) == [1, 2, 3], list(dq)
assert deep.maxlen == 5, deep.maxlen

own = dq.copy()
own.append(9)
assert list(dq) == [1, 2, 3], list(dq)
assert own.maxlen == 5, own.maxlen
assert copy.copy(deque([1], maxlen=None)).maxlen is None

# maxlen still evicts on the copy.
bounded = copy.copy(deque([1, 2, 3], maxlen=3))
bounded.append(4)
assert list(bounded) == [2, 3, 4], list(bounded)

shared = [1]
dq = deque([shared])
assert copy.copy(dq)[0] is shared
copy.deepcopy(dq)[0].append(2)
assert shared == [1], shared

# A deque copied mid-iteration gets its own mutation counter: appending to the
# copy must not disturb the original's live iterator.
src = deque([1, 2, 3])
seen = []
for value in src:
    if value == 1:
        clone = copy.copy(src)
        clone.append(99)
    seen.append(value)
assert seen == [1, 2, 3], seen

# …while the copy still guards its own iteration.
clone = copy.copy(deque([1, 2, 3]))
try:
    for value in clone:
        clone.append(9)
except RuntimeError as exc:
    print("deque guard:", exc)

# ── defaultdict ──────────────────────────────────────────────────────────────


def factory():
    return 7


dd = defaultdict(factory, a=1)
shallow = copy.copy(dd)
shallow["z"] = 9
assert sorted(dd) == ["a"], sorted(dd)
assert shallow.default_factory is factory
assert shallow["missing"] == 7
assert sorted(dd) == ["a"], sorted(dd)

deep = copy.deepcopy(dd)
deep["z"] = 9
assert sorted(dd) == ["a"], sorted(dd)
assert deep.default_factory is factory

own = dd.copy()
own["z"] = 9
assert sorted(dd) == ["a"], sorted(dd)
assert own.default_factory is factory

assert copy.copy(defaultdict(None, a=1)).default_factory is None

shared = [1]
dd = defaultdict(list, x=shared)
assert copy.copy(dd)["x"] is shared

# ── Counter ──────────────────────────────────────────────────────────────────

ct = Counter(a=1)
shallow = copy.copy(ct)
shallow["z"] = 9
assert sorted(ct) == ["a"], sorted(ct)
assert sorted(shallow) == ["a", "z"], sorted(shallow)

for clone in (copy.deepcopy(Counter(a=1)), Counter(a=1).copy(), Counter(Counter(a=1))):
    clone["z"] = 9
    assert sorted(clone) == ["a", "z"], sorted(clone)

# ── user subclasses of the primitive containers ──────────────────────────────


class MyList(list):
    pass


class MyDict(dict):
    pass


class MySet(set):
    pass


class MyBytearray(bytearray):
    pass


class MyOrderedDict(OrderedDict):
    pass


ml = MyList([1, 2])
ml.tag = "kept"
clone = copy.copy(ml)
clone.append(3)
assert list(ml) == [1, 2], list(ml)
assert type(clone) is MyList
assert clone.tag == "kept"

md = MyDict(a=1)
clone = copy.copy(md)
clone["z"] = 9
assert sorted(md) == ["a"], sorted(md)
assert type(clone) is MyDict

ms = MySet([1])
clone = copy.copy(ms)
clone.add(9)
assert sorted(ms) == [1], sorted(ms)
assert type(clone) is MySet

mb = MyBytearray(b"ab")
clone = copy.copy(mb)
clone.append(99)
assert bytes(mb) == b"ab", bytes(mb)
assert bytes(clone) == b"abc", bytes(clone)
assert type(clone) is MyBytearray

mo = MyOrderedDict(a=1)
clone = copy.copy(mo)
clone["z"] = 9
assert list(mo) == ["a"], list(mo)
assert type(clone) is MyOrderedDict

# deepcopy of the same family
for source, mutate in (
    (MyList([1]), lambda o: o.append(2)),
    (MyDict(a=1), lambda o: o.__setitem__("z", 9)),
    (MySet([1]), lambda o: o.add(9)),
    (MyBytearray(b"a"), lambda o: o.append(99)),
):
    before = len(source)
    mutate(copy.deepcopy(source))
    assert len(source) == before, (type(source).__name__, len(source))

# A custom __getstate__ covers the instance attributes, never the container's
# own payload — the copy still gets the original's contents, independently.
# This holds for deepcopy exactly as for copy: a state that omits the payload
# must not leave the copy without one (a backing-less dict subclass is broken,
# not empty).


class Stateful(dict):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.note = "n"

    def __getstate__(self):
        return {"note": self.note + "!"}

    def __setstate__(self, state):
        self.note = state["note"]


stateful = Stateful(a=1)
clone = copy.copy(stateful)
clone["z"] = 9
assert sorted(stateful) == ["a"], sorted(stateful)
assert sorted(clone) == ["a", "z"], sorted(clone)
assert clone.note == "n!"

deep = copy.deepcopy(stateful)
assert len(deep) == 1, len(deep)
assert sorted(deep.items()) == [("a", 1)], sorted(deep.items())
assert deep.note == "n!"
deep["z"] = 9
assert sorted(stateful) == ["a"], sorted(stateful)


class StatefulList(list):
    def __init__(self, *args):
        super().__init__(*args)
        self.note = "n"

    def __getstate__(self):
        return {"note": self.note + "!"}

    def __setstate__(self, state):
        self.note = state["note"]


stateful_list = StatefulList([1, 2])
for clone in (copy.copy(stateful_list), copy.deepcopy(stateful_list)):
    assert list(clone) == [1, 2], list(clone)
    assert clone.note == "n!"
    clone.append(3)
    assert list(stateful_list) == [1, 2], list(stateful_list)


class StatefulOrdered(OrderedDict):
    def __getstate__(self):
        return {"tag": 1}

    def __setstate__(self, state):
        self.tag = state["tag"]


stateful_ordered = StatefulOrdered(a=1, b=2)
for clone in (copy.copy(stateful_ordered), copy.deepcopy(stateful_ordered)):
    assert list(clone) == ["a", "b"], list(clone)
    assert clone.tag == 1
    clone["z"] = 9
    assert list(stateful_ordered) == ["a", "b"], list(stateful_ordered)


# A deque subclass whose state omits the opaque `_items` storage: the deep copy
# must still carry the elements, and independently.
class StatefulDeque(deque):
    def __getstate__(self):
        return {}

    def __setstate__(self, state):
        pass


stateful_deque = StatefulDeque([1, 2])
for clone in (copy.copy(stateful_deque), copy.deepcopy(stateful_deque)):
    assert list(clone) == [1, 2], list(clone)
    clone.append(3)
    assert list(stateful_deque) == [1, 2], list(stateful_deque)

# ── plain bytearray ──────────────────────────────────────────────────────────

ba = bytearray(b"ab")
clone = copy.copy(ba)
clone.append(99)
assert bytes(ba) == b"ab", bytes(ba)
deep = copy.deepcopy(ba)
deep.append(99)
assert bytes(ba) == b"ab", bytes(ba)
assert bytes(copy.copy(bytearray())) == b""

# ── an instance __dict__ copies into a detached dict ─────────────────────────


class Plain:
    pass


obj = Plain()
obj.a = [1]
mapping = copy.copy(obj.__dict__)
assert type(mapping) is dict, type(mapping)
assert mapping["a"] is obj.a
mapping["b"] = 2
assert sorted(obj.__dict__) == ["a"], sorted(obj.__dict__)

deep = copy.deepcopy(obj.__dict__)
assert type(deep) is dict, type(deep)
deep["a"].append(2)
assert obj.a == [1], obj.a

# Copying the object itself is unchanged: shallow shares the attribute values,
# deep does not.
shallow_obj = copy.copy(obj)
assert shallow_obj.a is obj.a
deep_obj = copy.deepcopy(obj)
deep_obj.a.append(2)
assert obj.a == [1], obj.a

# ── immutable / scalar backings are shared, as CPython does ──────────────────


class MyStr(str):
    pass


class MyInt(int):
    pass


class MyTuple(tuple):
    pass


class MyFrozenset(frozenset):
    pass


assert copy.copy(MyStr("ab")) == "ab"
assert copy.copy(MyInt(3)) + 1 == 4
assert tuple(copy.copy(MyTuple((1, 2)))) == (1, 2)
assert sorted(copy.copy(MyFrozenset([1, 2]))) == [1, 2]
assert copy.deepcopy(MyStr("ab")) == "ab"

# ── cycles and sharing still behave under deepcopy ───────────────────────────

selfish = deque([1])
selfish.append(selfish)
clone = copy.deepcopy(selfish)
assert clone[1] is clone
assert clone[1] is not selfish

inner = deque([1])
outer = [inner, inner]
clone = copy.deepcopy(outer)
assert clone[0] is clone[1]
assert clone[0] is not inner

# ── plain built-in containers are unchanged ──────────────────────────────────

source = [1, [2]]
clone = copy.copy(source)
assert clone is not source and clone[1] is source[1]
assert copy.copy((1, 2)) == (1, 2)
assert copy.copy(frozenset({1})) == frozenset({1})
assert copy.deepcopy(source)[1] is not source[1]

print("ok")
