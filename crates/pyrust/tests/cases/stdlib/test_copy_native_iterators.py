# copy.copy / copy.deepcopy of the native iterator types (#2974).
#
# CPython copies every iterator through its own __reduce__, and the built-ins
# split into two shapes:
#
#   * sequence-shaped — (iter, (sequence,), index): the copy retains the *same*
#     sequence and resumes at the same index, so both cursors see later writes
#     to that sequence;
#   * cursor-shaped — (iter, ([remaining, ...],)): a dict/set cursor cannot be
#     resumed by index, so the reduction drains what is left into a plain list
#     and the copy is a *list_iterator* that has left the mapping entirely.
#
# Generators reduce to nothing and raise TypeError.

import copy


def rest(it):
    return list(it)


# ── sequence-shaped: independent cursor over a shared sequence ───────────────

for make in (
    lambda: iter([1, 2, 3, 4]),
    lambda: iter((1, 2, 3, 4)),
    lambda: iter("abcd"),
    lambda: iter(b"abcd"),
    lambda: iter(range(4)),
    lambda: iter(bytearray(b"abcd")),
    lambda: reversed([1, 2, 3, 4]),
    lambda: reversed((1, 2, 3, 4)),
    lambda: reversed("abcd"),
    lambda: reversed(range(4)),
):
    it = make()
    next(it)
    c = copy.copy(it)
    assert c is not it
    # Both resume from the same position and neither moves the other.
    assert next(c) == next(it)
    assert rest(c) == rest(make())[2:]

# The copy's type is the source iterator's type, not a generic one.
print(type(copy.copy(iter([1, 2]))).__name__)
print(type(copy.copy(iter((1, 2)))).__name__)
print(type(copy.copy(iter("ab"))).__name__)
print(type(copy.copy(iter(b"ab"))).__name__)
print(type(copy.copy(iter(range(2)))).__name__)
print(type(copy.copy(reversed([1, 2]))).__name__)
print(type(copy.copy(reversed((1, 2)))).__name__)

# A shallow copy shares the sequence: writes are visible to both cursors.
data = [1, 2, 3, 4]
it = iter(data)
next(it)
c = copy.copy(it)
data[2] = 99
assert next(it) == 2 and next(c) == 2
data.append(5)
assert rest(it) == [99, 4, 5]
assert rest(c) == [99, 4, 5]

# `reversed` captured its length at construction, so an append stays invisible
# to the copy exactly as it does to the original.
data = [1, 2, 3]
it = reversed(data)
next(it)
c = copy.copy(it)
data.append(9)
assert rest(c) == [2, 1]
assert rest(it) == [2, 1]

# A str iterator over non-ASCII text copies by codepoint, not byte.
it = iter("aあ\U0001F600b")
next(it)
assert rest(copy.copy(it)) == ["あ", "\U0001F600", "b"]

# A large range never materialises, and its copy resumes lazily.
it = iter(range(10**18))
next(it)
c = copy.copy(it)
assert next(c) == 1 and next(it) == 1

# An arbitrary-precision range keeps the same contract.
it = iter(range(2**63, 2**63 + 4))
next(it)
c = copy.copy(it)
assert next(c) == 2**63 + 1
assert next(it) == 2**63 + 1


# ── cursor-shaped: the copy is a list_iterator over the remaining items ──────

d = {1: "a", 2: "b", 3: "c", 4: "d"}
for make, expected in (
    (lambda: iter(d), [2, 3, 4]),
    (lambda: iter(d.keys()), [2, 3, 4]),
    (lambda: iter(d.values()), ["b", "c", "d"]),
    (lambda: iter(d.items()), [(2, "b"), (3, "c"), (4, "d")]),
    (lambda: reversed(d), [3, 2, 1]),
    (lambda: reversed(d.values()), ["c", "b", "a"]),
    (lambda: reversed(d.items()), [(3, "c"), (2, "b"), (1, "a")]),
    (lambda: iter({1, 2, 3, 4}), None),
    (lambda: iter(frozenset({1, 2, 3, 4})), None),
):
    it = make()
    first = next(it)
    c = copy.copy(it)
    # Draining the copy leaves the original where it stood.
    assert type(c).__name__ == "list_iterator"
    drained = rest(c)
    assert rest(it) == drained
    if expected is not None:
        assert drained == expected

# The copy walks a *list*, so it is detached from the mapping: a later size
# change raises RuntimeError at the original and is invisible to the copy.
d = {1: "a", 2: "b", 3: "c", 4: "d"}
it = iter(d)
next(it)
c = copy.copy(it)
d[5] = "e"
try:
    next(it)
    raise AssertionError("original must report the size change")
except RuntimeError as e:
    print("dict original:", e)
assert rest(c) == [2, 3, 4]

# Clearing the mapping likewise cannot reach the copy.
d = {1: "a", 2: "b", 3: "c"}
it = iter(d)
c = copy.copy(it)
d.clear()
assert rest(c) == [1, 2, 3]
try:
    next(it)
    raise AssertionError("original must report the size change")
except RuntimeError:
    pass

s = {1, 2, 3}
it = iter(s)
next(it)
c = copy.copy(it)
s.add(99)
assert len(rest(c)) == 2
try:
    next(it)
    raise AssertionError("original must report the size change")
except RuntimeError as e:
    print("set original:", e)

# A cursor that already latched its size-change error re-raises out of the
# copy too — CPython's reduction drains a struct copy and hits the same latch.
d = {1: "a", 2: "b", 3: "c"}
it = iter(d)
next(it)
d[9] = "z"
try:
    next(it)
except RuntimeError:
    pass
try:
    copy.copy(it)
    raise AssertionError("copying a latched cursor must re-raise")
except RuntimeError as e:
    print("latched dict copy:", e)

s = {1, 2, 3}
it = iter(s)
next(it)
s.add(9)
try:
    next(it)
except RuntimeError:
    pass
try:
    copy.copy(it)
    raise AssertionError("copying a latched cursor must re-raise")
except RuntimeError as e:
    print("latched set copy:", e)

# The original keeps its own latch after the copy is taken.
d = {1: "a", 2: "b", 3: "c"}
it = iter(d)
next(it)
c = copy.copy(it)
d[4] = "d"
for _ in range(2):
    try:
        next(it)
        raise AssertionError("the latch must keep re-raising")
    except RuntimeError:
        pass
assert rest(c) == [2, 3]

# Reducing a cursor drains a *probe*, and the probe must not retire the
# terminal-key removal watch the original still relies on: after any number of
# copies the original still separates a delete/reinsert of its final key from an
# unrelated churn.
d = {"a": 1}
it = iter(d)
next(it)
for _ in range(5):
    copy.copy(it)
del d["a"]
d["a"] = 1
try:
    rest(it)
    raise AssertionError("the reinserted terminal key must still be detected")
except RuntimeError as e:
    print("watch survived copies:", e)

# The same watch must not fire for an insert/remove that leaves it alone.
d = {"a": 1}
it = iter(d)
next(it)
for _ in range(5):
    copy.copy(it)
d["tmp"] = 0
del d["tmp"]
assert rest(it) == []

# Two cursors over one mapping, each copied, keep their own watches.
d = {"a": 1, "b": 2}
ahead, behind = iter(d), iter(d)
next(ahead)
next(ahead)
next(behind)
assert rest(copy.copy(ahead)) == []
assert rest(copy.copy(behind)) == ["b"]
del d["b"]
d["b"] = 22
try:
    rest(ahead)
    raise AssertionError("the finished cursor must report the reinsertion")
except RuntimeError:
    pass
assert rest(behind) == ["b"]


# Copying the copy stays independent, and it is a plain list walk by then.
d = {1: "a", 2: "b", 3: "c"}
it = iter(d)
next(it)
c1 = copy.copy(it)
c2 = copy.copy(c1)
next(c1)
assert next(c2) == 2
assert rest(c2) == [3]


# ── source-holding adapters: the copy shares the source iterator ─────────────

src = iter([1, 2, 3, 4])
e = enumerate(src)
next(e)
c = copy.copy(e)
# The counter is the adapter's own state; the element source is shared.
assert next(c) == (1, 2)
assert next(e) == (1, 3)

for make in (
    lambda s: zip(s, [5, 6, 7, 8]),
    lambda s: map(lambda x: x * 2, s),
    lambda s: filter(None, s),
):
    src = iter([1, 2, 3, 4])
    it = make(src)
    next(it)
    c = copy.copy(it)
    # One shared source: the two adapters interleave rather than repeat.
    a, b = next(c), next(it)
    assert a != b

# deepcopy copies the source, so the adapters become fully independent.
src = iter([1, 2, 3, 4])
e = enumerate(src)
next(e)
d = copy.deepcopy(e)
assert next(d) == (1, 2)
assert next(e) == (1, 2)


# ── exhausted iterators copy to empty iterators ──────────────────────────────

for make in (
    lambda: iter([1, 2]),
    lambda: iter((1, 2)),
    lambda: iter("ab"),
    lambda: iter(b"ab"),
    lambda: iter(range(2)),
    lambda: reversed([1, 2]),
    lambda: iter({1: "a", 2: "b"}),
    lambda: iter({1, 2}),
    lambda: enumerate([1, 2]),
    lambda: zip([1, 2], [3, 4]),
    lambda: map(str, [1, 2]),
    lambda: filter(None, [1, 2]),
):
    it = make()
    rest(it)
    c = copy.copy(it)
    assert rest(c) == []
    try:
        next(it)
        raise AssertionError("the exhausted original must stay exhausted")
    except StopIteration:
        pass

# Exhaustion does not change the *shape* of the reduction: a cursor still
# reduces to `(iter, ([],))`, so its copy is a list_iterator with an empty list
# rather than another cursor over the mapping it already left.
mapping = {1: "a", 2: "b"}


class Holder:
    pass


holder = Holder()
holder.x = 1
for make in (
    lambda: iter(mapping),
    lambda: iter(mapping.keys()),
    lambda: iter(mapping.values()),
    lambda: iter(mapping.items()),
    lambda: reversed(mapping),
    lambda: reversed(mapping.values()),
    lambda: reversed(mapping.items()),
    lambda: iter({1, 2}),
    lambda: iter(frozenset({1, 2})),
    lambda: iter(holder.__dict__),
):
    it = make()
    rest(it)
    c = copy.copy(it)
    assert type(c).__name__ == "list_iterator", (make, type(c).__name__)
    assert rest(c) == []
    assert c.__length_hint__() == 0

# A sequence walk keeps its own type across exhaustion.
for make, name in (
    (lambda: iter([1, 2]), "list_iterator"),
    (lambda: iter((1, 2)), "tuple_iterator"),
    (lambda: iter(range(2)), "range_iterator"),
    (lambda: reversed([1, 2]), "list_reverseiterator"),
    (lambda: reversed((1, 2)), "reversed"),
):
    it = make()
    rest(it)
    assert type(copy.copy(it)).__name__ == name

# Empty sources copy to empty iterators.
for make in (
    lambda: iter([]),
    lambda: iter(()),
    lambda: iter(""),
    lambda: iter(range(0)),
    lambda: iter({}),
    lambda: iter(set()),
    lambda: reversed([]),
):
    assert rest(copy.copy(make())) == []


# ── deepcopy copies the retained source ──────────────────────────────────────

nested = [[1], [2], [3]]
it = iter(nested)
next(it)
shallow = copy.copy(it)
deep = copy.deepcopy(iter(nested))
next(deep)
assert next(shallow) is nested[1]
assert next(deep) == [2]
assert next(deep) is not nested[2]

# The deep copy walks its own list, so a later append is invisible to it.
nested = [[1], [2], [3]]
it = iter(nested)
next(it)
deep = copy.deepcopy(it)
nested.append([9])
assert rest(deep) == [[2], [3]]

# A drained cursor's remaining items are deep-copied too.
mapping = {1: [1], 2: [2], 3: [3]}
it = iter(mapping.values())
next(it)
deep = copy.deepcopy(it)
assert next(deep) == [2]
assert next(deep) is not mapping[3]

# A bytearray iterator's buffer is shared shallowly and detached deeply.
buf = bytearray(b"abcd")
it = iter(buf)
next(it)
shallow = copy.copy(it)
deep = copy.deepcopy(iter(buf))
next(deep)
buf[1] = 90
assert next(shallow) == 90
assert next(deep) == 98

# A structure that reaches its own iterator still terminates.
holder = []
it = iter(holder)
holder.append(it)
d = copy.deepcopy(it)
assert type(d).__name__ == "list_iterator"

# The memo keeps one copy of a source shared by two iterators.
seq = [1, 2, 3]
d1, d2 = copy.deepcopy((iter(seq), iter(seq)))
next(d1)
assert next(d2) == 1


# ── __length_hint__ survives the copy ────────────────────────────────────────

for make in (
    lambda: iter([1, 2, 3, 4]),
    lambda: iter(range(4)),
    lambda: iter({1: 1, 2: 2, 3: 3, 4: 4}),
    lambda: iter({1, 2, 3, 4}),
    lambda: reversed([1, 2, 3, 4]),
):
    it = make()
    next(it)
    assert copy.copy(it).__length_hint__() == it.__length_hint__() == 3


# ── generators are not copyable ──────────────────────────────────────────────


def gen():
    yield 1
    yield 2


for op in (copy.copy, copy.deepcopy):
    try:
        op(gen())
        raise AssertionError("a generator must not be copyable")
    except TypeError as e:
        print(e)

    try:
        op(x for x in [1, 2])
        raise AssertionError("a genexpr must not be copyable")
    except TypeError as e:
        print(e)

# Copying consults the reduction, never the frame, so a generator that is
# *running* when it is copied is refused exactly like a suspended one — a
# TypeError, not a re-entrancy error.
running = []


def copies_itself():
    for op in (copy.copy, copy.deepcopy):
        try:
            op(self_gen)
            running.append("copied")
        except TypeError as e:
            running.append(str(e))
    yield 1


self_gen = copies_itself()
next(self_gen)
print(running)

# Same through the for-loop trampoline, which parks the frame elsewhere.
driven = []


def copies_itself_in_loop():
    try:
        copy.copy(loop_gen)
        driven.append("copied")
    except TypeError as e:
        driven.append(str(e))
    yield 1


loop_gen = copies_itself_in_loop()
for _ in loop_gen:
    break
print(driven)

# The refusal names the exact type even mid-run: the kind tag lives outside the
# checked-out frame, so a running coroutine / async generator is no longer
# lumped in under the "generator" noun (issue #2978).
import asyncio


async def coro_copies_itself():
    try:
        copy.copy(the_coro)
        return "copied"
    except TypeError as e:
        return str(e)


the_coro = coro_copies_itself()
print(asyncio.run(the_coro))


async def agen_copies_itself():
    try:
        copy.copy(the_agen)
        yield "copied"
    except TypeError as e:
        yield str(e)


the_agen = agen_copies_itself()


async def drive_agen():
    return await the_agen.__anext__()


print(asyncio.run(drive_agen()))

print("ok")
