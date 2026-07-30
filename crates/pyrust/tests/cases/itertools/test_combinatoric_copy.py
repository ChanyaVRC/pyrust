# copy.copy / copy.deepcopy of the combinatoric iterators (issue #2952).
#
# CPython reconstructs these through __reduce__ — (type, (pool, r)) plus a
# setstate of the index vector — so a copy taken mid-iteration resumes at
# exactly the original's position and the two iterators then advance
# independently in either order.  The pool is materialised once and never
# mutated, which is why even the *shallow* copy is fully independent here,
# unlike the adapters that hold a live source iterator.
#
# Stable across 3.11-3.13; CPython 3.14 removes itertools copy/pickle support
# altogether.
import copy
import itertools

BUILDERS = [
    ("combinations", lambda: itertools.combinations(range(4), 2)),
    ("permutations", lambda: itertools.permutations(range(3), 2)),
    ("product", lambda: itertools.product(range(2), range(2))),
    ("cwr", lambda: itertools.combinations_with_replacement(range(3), 2)),
]


def advanced(build, steps):
    iterator = build()
    for _ in range(steps):
        next(iterator)
    return iterator


print("== copy resumes at the original's position ==")
for label, build in BUILDERS:
    for steps in (0, 1, 2):
        original = advanced(build, steps)
        shallow = copy.copy(original)
        deep = copy.deepcopy(original)
        print(
            label,
            steps,
            list(original) == list(shallow) == list(deep),
            list(copy.copy(advanced(build, steps))),
        )

print()
print("== exhausted copies stay exhausted ==")


def step(iterator):
    try:
        return next(iterator)
    except StopIteration:
        return "STOP"


for label, build in BUILDERS:
    original = build()
    for _ in original:
        pass
    shallow = copy.copy(original)
    deep = copy.deepcopy(original)
    print(label, list(shallow), list(deep), step(shallow), step(deep))

print()
print("== advancing one does not move the other ==")
for label, build in BUILDERS:
    original = build()
    shallow = copy.copy(original)
    next(original)
    print(label, "orig-then-copy", next(shallow))

    original = build()
    shallow = copy.copy(original)
    next(shallow)
    print(label, "copy-then-orig", next(original))

print()
print("== draining order is irrelevant ==")
for label, build in BUILDERS:
    original = advanced(build, 1)
    shallow = copy.copy(original)
    copy_first = list(shallow)
    print(label, copy_first == list(original))

print()
print("== a copy is a new object of the same type ==")
for label, build in BUILDERS:
    original = build()
    shallow = copy.copy(original)
    deep = copy.deepcopy(original)
    print(
        label,
        shallow is original,
        deep is original,
        type(shallow) is type(original),
        type(deep) is type(original),
    )

print()
print("== shallow shares pooled elements, deep copies them ==")
pool = [[1], [2]]
original = itertools.product(pool, repeat=1)
shallow_element = next(copy.copy(original))[0]
deep_element = next(copy.deepcopy(original))[0]
print("shallow shares:", shallow_element is pool[0])
print("deep shares:", deep_element is pool[0])
print("deep equals:", deep_element == pool[0])
pool[0].append(99)
print("original pool mutation visible to deep copy:", deep_element)

elements = [[1], [2]]
original = itertools.combinations(elements, 2)
first_shallow = next(copy.copy(original))
first_deep = next(copy.deepcopy(original))
print("combinations shallow shares:", first_shallow[0] is elements[0])
print("combinations deep shares:", first_deep[0] is elements[0])
print("combinations deep equals:", first_deep == ([1], [2]))

print()
print("== degenerate cursors copy too ==")
DEGENERATE = [
    ("comb r=0", lambda: itertools.combinations(range(3), 0)),
    ("perm r=0", lambda: itertools.permutations(range(3), 0)),
    ("cwr r=0", lambda: itertools.combinations_with_replacement(range(3), 0)),
    ("product()", lambda: itertools.product()),
    ("product repeat=0", lambda: itertools.product([1, 2], repeat=0)),
    ("comb r>n", lambda: itertools.combinations(range(2), 5)),
    ("perm r>n", lambda: itertools.permutations(range(2), 5)),
    ("cwr empty pool", lambda: itertools.combinations_with_replacement([], 2)),
    ("product empty dim", lambda: itertools.product([], [1])),
    ("comb empty pool", lambda: itertools.combinations([], 0)),
]
for label, build in DEGENERATE:
    original = build()
    shallow = copy.copy(original)
    deep = copy.deepcopy(original)
    print(label, list(original), list(shallow), list(deep))

print()
print("== degenerate cursors after the single yield is taken ==")
for label, build in DEGENERATE[:5]:
    original = build()
    step(original)
    shallow = copy.copy(original)
    print(label, list(shallow), list(original))

print()
print("== repeated product dimensions stay one shared pool ==")
original = itertools.product("AB", repeat=3)
next(original)
shallow = copy.copy(original)
deep = copy.deepcopy(original)
print(len(list(shallow)), len(list(deep)), len(list(original)))

original = itertools.product("AB", repeat=2)
print(list(copy.deepcopy(original)))

print()
print("== copies are independently re-copyable ==")
original = advanced(lambda: itertools.combinations(range(5), 2), 3)
first = copy.copy(original)
second = copy.copy(first)
third = copy.deepcopy(second)
print(list(first) == list(second) == list(third) == list(original))

print()
print("== fully independent simple iterators keep working ==")
for label, build, steps in [
    ("count", lambda: itertools.count(5), 2),
    ("repeat", lambda: itertools.repeat(7, 4), 1),
    ("cycle", lambda: itertools.cycle([1, 2, 3]), 4),
]:
    original = advanced(build, steps)
    shallow = copy.copy(original)
    deep = copy.deepcopy(original)
    print(
        label,
        list(itertools.islice(shallow, 4)),
        list(itertools.islice(deep, 4)),
        list(itertools.islice(original, 4)),
    )
