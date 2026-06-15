# itertools iterator types are real `type` objects (issue #2370).
# `chain` in particular used to be a builtin function, so
# `type(chain(...))` reported `<built-in function itertools.chain>`
# instead of `<class 'itertools.chain'>`.
from itertools import chain
import itertools

# chain is a real class.
print(type(chain([1])))
print(type(chain([1, 2])).__name__)
print(chain.__name__)
print(chain.__qualname__)
print(chain.__module__)
print(itertools.chain is chain)

# type identity across the two constructors.
print(type(chain([1])) is type(chain.from_iterable([[1]])))
print(type(chain.from_iterable([[1]])).__name__)
print(type(chain.from_iterable([[1]])).__module__)

# isinstance flows through the standard class machinery.
print(isinstance(chain([1]), type(chain([1]))))
print(isinstance(chain([1]), chain))
print(isinstance(chain.from_iterable([[1]]), chain))
print(isinstance([1], chain))

# Every itertools iterator type reports as a `type`.
names = [
    "chain", "islice", "count", "repeat", "cycle", "takewhile",
    "dropwhile", "starmap", "accumulate", "product", "combinations",
    "combinations_with_replacement", "permutations", "groupby",
    "compress", "zip_longest", "filterfalse", "pairwise", "batched",
]
for name in names:
    print(name, type(getattr(itertools, name)).__name__)

# Iteration behaviour is unchanged.
print(list(chain([1, 2], [3, 4], (5,))))
print(list(chain()))
print(list(chain([])))
print(list(chain("ab", "cd")))
print(list(chain.from_iterable([[1, 2], [3], [4, 5]])))
print(list(chain.from_iterable([])))

# Lazy across sources: from_iterable over an unbounded outer source,
# truncated with islice, must not over-consume.
print(list(itertools.islice(
    chain.from_iterable(range(i, i + 2) for i in itertools.count(0, 10)), 5
)))

# from_iterable accepts a chain instance as its (single) iterable arg.
print(list(chain.from_iterable(chain([[9], [8]]))))

# from_iterable callable on an instance too (classmethod semantics).
print(list(chain([1]).from_iterable([[7], [6]])))

# An exhausted chain stays exhausted.
c = chain([1], [2])
print(list(c), list(c))

# Subclassing works through the normal class machinery.
class MyChain(chain):
    pass

print(list(MyChain([1], [2])))
print(type(MyChain([1], [2])).__name__)

# Error paths.
try:
    chain(foo=1)
except TypeError as e:
    print("kw:", e)
try:
    chain.from_iterable()
except TypeError as e:
    print("fi0:", e)
try:
    chain.from_iterable([1], [2])
except TypeError as e:
    print("fi2:", e)
# A 2-arg *unbound* call whose first arg is a user iterator is still an
# arity error — the receiver-strip must not swallow it (the count-based
# discriminator alone would have).
class _It:
    def __iter__(self):
        return iter([[1], [2]])
try:
    chain.from_iterable(_It(), _It())
except TypeError as e:
    print("fi-unbound2:", e)
# The bound-form arity error reports the user-visible arg count (2), not
# the count inflated by the silently-prepended receiver.
try:
    chain([]).from_iterable([[1]], [[2]])
except TypeError as e:
    print("fi-bound2:", e)
