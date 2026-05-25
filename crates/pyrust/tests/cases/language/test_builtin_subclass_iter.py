"""
Iterating over list/dict/set subclass instances via GetIter, collect_iterable,
and iter_values should delegate to the backing primitive, matching CPython.
"""


class MyList(list):
    pass


class MyDict(dict):
    pass


class MySet(set):
    pass


# for loop over list subclass
for x in MyList([1, 2, 3]):
    print(x)

# for loop over dict subclass (yields keys in insertion order)
for k in MyDict({"a": 1, "b": 2, "c": 3}):
    print(k)

# for loop over set subclass (order varies; sort to stabilise output)
for x in sorted(MySet({10, 20, 30})):
    print(x)

# list() constructor on subclasses
print(list(MyList([4, 5, 6])))
print(list(MyDict({"x": 1, "y": 2})))
print(sorted(list(MySet({6, 4, 5}))))

# tuple() constructor on list subclass
print(tuple(MyList([7, 8])))

# sum and sorted on list subclass
print(sum(MyList([10, 20, 30])))
print(sorted(MyList([3, 1, 2])))

# empty subclasses
print(list(MyList([])))
print(list(MyDict({})))
print(sorted(list(MySet(set()))))

# user-defined __iter__ in subclass overrides backing iteration
class OverrideList(list):
    def __iter__(self):
        yield 99
        yield 100


for x in OverrideList([1, 2, 3]):
    print(x)

print(list(OverrideList([1, 2, 3])))
