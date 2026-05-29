"""Parity fixture: itertools class constructors produce CPython-matching arity errors."""
import itertools

# islice: needs at least 2 args (iterable + stop), at most 4 (iterable, start, stop, step)
try:
    itertools.islice()
except TypeError as e:
    print(e)  # islice expected at least 2 arguments, got 0

try:
    itertools.islice([1, 2, 3])
except TypeError as e:
    print(e)  # islice expected at least 2 arguments, got 1

try:
    itertools.islice([1], 0, 1, 1, 1)
except TypeError as e:
    print(e)  # islice expected at most 4 arguments, got 5

# count: optional start and step
try:
    itertools.count(1, 2, 3)
except TypeError as e:
    print(e)  # count() takes at most 2 arguments (3 given)

# repeat: 'object' is required, 'times' is optional
try:
    itertools.repeat()
except TypeError as e:
    print(e)  # repeat() missing required argument 'object' (pos 1)

try:
    itertools.repeat(1, 2, 3)
except TypeError as e:
    print(e)  # repeat() takes at most 2 arguments (3 given)

# cycle: exactly 1 arg
try:
    itertools.cycle()
except TypeError as e:
    print(e)  # cycle expected 1 argument, got 0

try:
    itertools.cycle([1], [2])
except TypeError as e:
    print(e)  # cycle expected 1 argument, got 2

# takewhile: predicate + iterable
try:
    itertools.takewhile(lambda x: x)
except TypeError as e:
    print(e)  # takewhile expected 2 arguments, got 1

try:
    itertools.takewhile(lambda x: x, [1], [2])
except TypeError as e:
    print(e)  # takewhile expected 2 arguments, got 3

# dropwhile: predicate + iterable
try:
    itertools.dropwhile(lambda x: x)
except TypeError as e:
    print(e)  # dropwhile expected 2 arguments, got 1

# starmap: function + iterable
try:
    itertools.starmap(lambda x: x)
except TypeError as e:
    print(e)  # starmap expected 2 arguments, got 1

# accumulate: iterable required, func optional
try:
    itertools.accumulate()
except TypeError as e:
    print(e)  # accumulate() missing required argument 'iterable' (pos 1)

try:
    itertools.accumulate([1], lambda a, b: a + b, lambda a, b: a - b)
except TypeError as e:
    print(e)  # accumulate() takes at most 2 positional arguments (3 given)

# permutations: iterable required, r optional
try:
    itertools.permutations()
except TypeError as e:
    print(e)  # permutations() missing required argument 'iterable' (pos 1)

try:
    itertools.permutations([1, 2], 1, 2)
except TypeError as e:
    print(e)  # permutations() takes at most 2 arguments (3 given)

# combinations: both iterable and r required
try:
    itertools.combinations([1, 2])
except TypeError as e:
    print(e)  # combinations() missing required argument 'r' (pos 2)

try:
    itertools.combinations()
except TypeError as e:
    print(e)  # combinations() missing required argument 'iterable' (pos 1)

try:
    itertools.combinations([1, 2], 1, 2)
except TypeError as e:
    print(e)  # combinations() takes at most 2 arguments (3 given)

# combinations_with_replacement: both iterable and r required
try:
    itertools.combinations_with_replacement([1, 2])
except TypeError as e:
    print(e)  # combinations_with_replacement() missing required argument 'r' (pos 2)

try:
    itertools.combinations_with_replacement([1, 2], 1, 2)
except TypeError as e:
    print(e)  # combinations_with_replacement() takes at most 2 arguments (3 given)

# compress: data + selectors required
try:
    itertools.compress()
except TypeError as e:
    print(e)  # compress() missing required argument 'data' (pos 1)

try:
    itertools.compress([1])
except TypeError as e:
    print(e)  # compress() missing required argument 'selectors' (pos 2)

try:
    itertools.compress([1], [1], [1])
except TypeError as e:
    print(e)  # compress() takes at most 2 arguments (3 given)

# Happy-path smoke check: correct arity should not raise
print(list(itertools.islice([1, 2, 3, 4], 2)))  # [1, 2]
print(list(itertools.islice([1, 2, 3, 4], 1, 3)))  # [2, 3]
print(next(itertools.count(5, 2)))  # 5
print(next(itertools.repeat(42)))  # 42
print(list(itertools.repeat(7, 3)))  # [7, 7, 7]
print(next(itertools.cycle([1, 2])))  # 1
print(list(itertools.takewhile(lambda x: x < 3, [1, 2, 3, 4])))  # [1, 2]
print(list(itertools.dropwhile(lambda x: x < 3, [1, 2, 3, 4])))  # [3, 4]
print(list(itertools.starmap(lambda x, y: x + y, [(1, 2), (3, 4)])))  # [3, 7]
print(list(itertools.accumulate([1, 2, 3, 4])))  # [1, 3, 6, 10]
print(list(itertools.permutations([1, 2, 3], 2)))  # [(1,2),(1,3),(2,1),(2,3),(3,1),(3,2)]
print(list(itertools.combinations([1, 2, 3], 2)))  # [(1,2),(1,3),(2,3)]
print(list(itertools.combinations_with_replacement([1, 2], 2)))  # [(1,1),(1,2),(2,2)]
print(list(itertools.compress([1, 2, 3], [1, 0, 1])))  # [1, 3]
