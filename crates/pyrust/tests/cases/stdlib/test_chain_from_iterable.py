# itertools.chain.from_iterable — the alternate constructor that flattens a
# single iterable whose elements are the iterables to chain.  Must be lazy:
# the outer iterable is consumed one element at a time and each inner iterable
# is iterated only when reached.  Issue #1920.

import itertools
from itertools import chain

# --- the three repro cases ---
print("strs", list(chain.from_iterable(["ab", "cd"])))
print("lists", list(chain.from_iterable([[1], [2, 3], []])))
print("empty", list(chain.from_iterable([])))

# --- mixed inner iterable kinds (str, list, tuple, range, generator) ---
print("mixed", list(chain.from_iterable(["ab", [1, 2], (3, 4), range(2), (x for x in [9, 8])])))

# --- outer source is itself an iterator (not a re-iterable container) ---
print("iter-outer", list(chain.from_iterable(iter([[1, 2], [3]]))))

# --- nested: from_iterable of a from_iterable ---
print("nested", list(chain.from_iterable(chain.from_iterable([[[1, 2], [3]], [[4]]]))))


# --- laziness probe: outer is a generator with side effects; inner iterables
# are pulled on demand, not all up front.  The "pull" prints interleave with
# the yielded values, proving the outer source is consumed lazily.
def gen():
    print("pull A")
    yield "AB"
    print("pull B")
    yield "CD"


it = chain.from_iterable(gen())
print("created")  # no "pull" printed yet — outer not consumed at construction
print("n", next(it))
print("n", next(it))
print("n", next(it))
print("n", next(it))
try:
    next(it)
except StopIteration:
    print("stop")

# --- generator-of-generators ---
def gens():
    for i in range(3):
        yield (j for j in range(i))


print("gen-of-gen", list(chain.from_iterable(gens())))

# --- iter(it) is it (the result is an iterator) ---
it2 = chain.from_iterable([[1]])
print("iter-is-self", iter(it2) is it2)

# --- a non-iterable element raises TypeError, but only when reached (lazy) ---
it3 = chain.from_iterable([[1, 2], 5, [3]])
print("before-error", next(it3), next(it3))
try:
    next(it3)
except TypeError as e:
    print("type-error", e)

# --- argument-count and keyword-argument errors ---
try:
    chain.from_iterable([1], [2])
except TypeError as e:
    print("too-many", e)
try:
    chain.from_iterable()
except TypeError as e:
    print("too-few", e)
try:
    chain.from_iterable(x=[1])
except TypeError as e:
    print("kwargs", e)

# --- introspection: attribute is accessible, chain() still works ---
print("hasattr", hasattr(chain, "from_iterable"))
print("chain-unaffected", list(itertools.chain([1, 2], [3], "ab")))
