# The common i64-backed range iterator must retain only its cursor. In
# particular, constructing iter/map/zip over a huge range must not materialise
# every element.

iterator = iter(range(1, 10, 3))
print("type:", type(iterator).__name__)
print("identity:", iter(iterator) is iterator)
print("next:", next(iterator), next(iterator))
print("remaining list:", list(iterator))

descending = iter(range(5, -2, -2))
seen = []
for value in descending:
    seen.append(value)
print("for:", seen)

empty = iter(range(3, 3))
print("empty:", next(empty, "done"))

huge = iter(range(10**9))
print(
    "huge prefix:",
    type(huge).__name__,
    next(huge),
    next(huge),
    next(huge),
)

mapped = map(lambda value: value + 10, range(10**9))
print("map prefix:", next(mapped), next(mapped))

zipped = zip(range(10**9), ("a", "b"))
print("zip finite peer:", list(zipped))

# Advancing the final item must not wrap an i64 cursor across the stop bound.
upper = 2**63 - 1
lower = -(2**63)
upper_edge = iter(range(upper - 1, upper, 2))
lower_edge = iter(range(lower + 1, lower, -2))
print("upper edge:", type(upper_edge).__name__, list(upper_edge))
print("lower edge:", type(lower_edge).__name__, list(lower_edge))
