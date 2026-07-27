from itertools import combinations
from itertools import combinations_with_replacement
from itertools import permutations
from itertools import product
import sys


def capture(label, callback):
    try:
        print(label + ":", callback())
    except Exception as exc:
        print(label + ":", type(exc).__name__, str(exc))


print("product order:", list(product("AB", [1, 2])))
print("product empty:", list(product()))
print("product repeat zero:", list(product([1, 2], repeat=0)))
print("combinations order:", list(combinations("ABCD", 2)))
print("replacement order:", list(combinations_with_replacement("ABC", 2)))
print("permutations order:", list(permutations("ABC", 2)))

print("combinations r zero:", list(combinations([], 0)))
print("combinations r too large:", list(combinations([1], 2)))
print("replacement r zero:", list(combinations_with_replacement([], 0)))
print("replacement empty:", list(combinations_with_replacement([], 2)))


def exhaustion_steps(iterator):
    values = []
    for _ in range(3):
        try:
            values.append(next(iterator))
        except StopIteration:
            values.append("STOP")
    return values


print("product exhausted:", exhaustion_steps(product([1])))
print("combinations exhausted:", exhaustion_steps(combinations([1], 1)))
print(
    "replacement exhausted:",
    exhaustion_steps(combinations_with_replacement([1], 1)),
)


class SinglePass:
    def __init__(self, values):
        self.values = values
        self.calls = 0

    def __iter__(self):
        self.calls += 1
        if self.calls != 1:
            raise RuntimeError("iterated twice")
        for value in self.values:
            yield value


product_source = SinglePass([1, 2])
product_iterator = product(product_source, repeat=3)
print("product materialized once:", product_source.calls, next(product_iterator))

zero_source = SinglePass([1, 2])
zero_iterator = product(zero_source, repeat=0)
print("product repeat zero untouched:", zero_source.calls, next(zero_iterator))

combination_source = SinglePass([1, 2, 3])
combination_iterator = combinations(combination_source, 2)
print(
    "combinations materialized once:",
    combination_source.calls,
    list(combination_iterator),
)

replacement_source = SinglePass([1, 2])
replacement_iterator = combinations_with_replacement(replacement_source, 2)
print(
    "replacement materialized once:",
    replacement_source.calls,
    list(replacement_iterator),
)

# These iterators are logically empty. PyRust must not allocate an r-sized
# index array merely to discover that fact.
huge_combinations = combinations([1], 1_000_000)
print("huge combinations exhausted:", exhaustion_steps(huge_combinations)[0])
del huge_combinations

huge_replacement = combinations_with_replacement([], 1_000_000)
print("huge replacement exhausted:", exhaustion_steps(huge_replacement)[0])
del huge_replacement

capture("product repeat overflow", lambda: product([1], repeat=sys.maxsize))
capture("product capacity overflow", lambda: product([1], [2], repeat=sys.maxsize))
capture("product allocation failure", lambda: product([1], repeat=10**18))
