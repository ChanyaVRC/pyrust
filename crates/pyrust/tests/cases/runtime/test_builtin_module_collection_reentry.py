# Builtin-module collection paths must release Rust-side read guards before
# dispatching user code.  The expected output pins CPython 3.12's mutation
# timing: list/dict repr walk live additions, while set repr snapshots members.

class MyList(list):
    pass


class ListGrow:
    def __init__(self, owner):
        self.owner = owner

    def __repr__(self):
        if len(self.owner) == 1:
            self.owner.append(2)
        return "LG"


values = MyList()
values.append(ListGrow(values))
print(list.__repr__(values))
print(len(values), values[-1])


class MyDict(dict):
    pass


class DictGrow:
    def __init__(self, owner):
        self.owner = owner

    def __repr__(self):
        if len(self.owner) == 1:
            self.owner["b"] = 2
        return "DG"


mapping = MyDict()
mapping["a"] = DictGrow(mapping)
print(dict.__repr__(mapping))
print(len(mapping), mapping["b"])


class MySet(set):
    pass


class SetGrow:
    def __init__(self, owner):
        self.owner = owner

    def __hash__(self):
        return 41

    def __repr__(self):
        self.owner.add(2)
        return "SG"


members = MySet()
members.add(SetGrow(members))
print(set.__repr__(members))
print(len(members), 2 in members)


# zip_longest keeps its iterator table in a Rust-backed List.  A user
# __next__ may recursively drive the same zip object, so each iterator Value
# must be cloned and its List guard released before dispatch.
from itertools import zip_longest


class Reenter:
    def __init__(self):
        self.calls = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.calls += 1
        if self.calls == 1:
            print("nested", next(zipped))
        if self.calls > 3:
            raise StopIteration
        return self.calls


zipped = zip_longest(Reenter(), [10, 20, 30], fillvalue="x")
while True:
    try:
        print("outer", next(zipped))
    except StopIteration:
        print("stop")
        break
