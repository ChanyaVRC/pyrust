# itertools iterators: `__iter__` returns self, and `__next__`
# raises StopIteration once exhausted (and stays exhausted on repeat
# calls).  Guards the #1895 dedup of the shared return-self `__iter__`
# and the `check_not_exhausted` exhaustion prologue.
import itertools as it


def iter_is_self(name, obj):
    print(name, iter(obj) is obj)


iter_is_self("count", it.count(0))
iter_is_self("repeat", it.repeat(1, 3))
iter_is_self("cycle", it.cycle([1, 2]))
iter_is_self("chain.from_iterable", it.chain.from_iterable([[1], [2]]))
iter_is_self("islice", it.islice(range(5), 2))
iter_is_self("takewhile", it.takewhile(lambda x: x < 3, range(5)))
iter_is_self("dropwhile", it.dropwhile(lambda x: x < 3, range(5)))
iter_is_self("starmap", it.starmap(pow, [(2, 3)]))
iter_is_self("accumulate", it.accumulate([1, 2, 3]))
iter_is_self("product", it.product([1, 2], [3, 4]))
iter_is_self("permutations", it.permutations([1, 2, 3], 2))
iter_is_self("combinations", it.combinations([1, 2, 3], 2))
iter_is_self("cwr", it.combinations_with_replacement([1, 2], 2))
iter_is_self("compress", it.compress([1, 2, 3], [1, 0, 1]))
iter_is_self("zip_longest", it.zip_longest([1], [2, 3]))
iter_is_self("filterfalse", it.filterfalse(lambda x: x % 2, range(5)))
iter_is_self("pairwise", it.pairwise([1, 2, 3]))
iter_is_self("batched", it.batched([1, 2, 3, 4], 2))

g = it.groupby([1, 1, 2])
iter_is_self("groupby", g)
_, grp = next(g)
iter_is_self("_grouper", grp)


# Full materialisation through the shared __next__ / StopIteration path.
print(list(it.islice(it.count(5), 3)))
print(list(it.takewhile(lambda x: x < 3, [1, 2, 3, 4, 1])))
print(list(it.dropwhile(lambda x: x < 3, [1, 2, 3, 1])))
print(list(it.product([1, 2], [3, 4])))
print(list(it.permutations([1, 2, 3], 2)))
print(list(it.combinations([1, 2, 3], 2)))
print(list(it.combinations_with_replacement([1, 2], 2)))
print(list(it.pairwise([1, 2, 3, 4])))
print(list(it.batched([1, 2, 3, 4, 5], 2)))


# Once exhausted, every subsequent next() keeps raising StopIteration.
def exhausted_stays_stopped(name, obj, advance):
    for _ in range(advance):
        next(obj)
    seen = 0
    for _ in range(3):
        try:
            next(obj)
        except StopIteration:
            seen += 1
    print(name, seen)


exhausted_stays_stopped("takewhile", it.takewhile(lambda x: x < 2, [1, 2]), 1)
exhausted_stays_stopped("dropwhile", it.dropwhile(lambda x: x < 2, [1, 2]), 1)
exhausted_stays_stopped("product", it.product([1], [2]), 1)
exhausted_stays_stopped("permutations", it.permutations([1, 2], 2), 2)
exhausted_stays_stopped("combinations", it.combinations([1, 2], 2), 1)
exhausted_stays_stopped("cwr", it.combinations_with_replacement([1], 1), 1)
exhausted_stays_stopped("pairwise", it.pairwise([1, 2]), 1)
exhausted_stays_stopped("batched", it.batched([1, 2], 2), 1)
