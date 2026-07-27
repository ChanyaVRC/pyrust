# CPython parity for the native permutations cursor.  These cases pin the
# positional algorithm and its edge-state transitions without depending on
# PyRust's internal representation.

from itertools import permutations


def pull(iterator):
    try:
        return next(iterator)
    except StopIteration:
        return "STOP"


print("r2", list(permutations("ABC", 2)))
print("default", list(permutations("ABC")))
print("repeated", list(permutations("AAB", 2)))
print("zero-nonempty", list(permutations("ABC", 0)))
print("zero-empty", list(permutations([], 0)))
print("too-large", list(permutations("AB", 5)))

stepwise = permutations("ABC", 2)
print("self", iter(stepwise) is stepwise)
print("step", pull(stepwise), pull(stepwise))
print("rest", list(stepwise))
print("stable-stop", pull(stepwise), pull(stepwise))

# The input is materialised exactly once by the constructor.  Later mutation
# of the original list cannot change the cursor's pool.
source = [1, 2, 3]
snapshot = permutations(source, 2)
source[0] = 99
print("snapshot", list(snapshot))


class Source:
    def __init__(self):
        self.iterations = 0

    def __iter__(self):
        self.iterations += 1
        return iter([1, 2, 3])


source = Source()
cursor = permutations(source, 2)
print("materialised", source.iterations)
print("custom-source", list(cursor), source.iterations)

