# Constant propagation must forget named-local facts whenever an opcode can
# execute Python or mutate a live namespace.  None of the mutations below is a
# Call instruction in the outer frame.


def namespace_owner():
    pass


shared = namespace_owner.__globals__


class NegMutator:
    def __neg__(self):
        shared["tracked"] = 30
        return 0


class AddMutator:
    def __add__(self, other):
        shared["tracked"] = 40
        return other


class AttrMutator:
    def __getattr__(self, name):
        shared["tracked"] = 50
        return name


class ItemMutator:
    def __getitem__(self, key):
        shared["tracked"] = 60
        return key


class FormatMutator:
    def __format__(self, spec):
        shared["tracked"] = 70
        return "formatted"


class HashMutator:
    def __hash__(self):
        shared["tracked"] = 80
        return 1


class SetItemMutator:
    def __setitem__(self, key, value):
        shared["tracked"] = 90


class TruthMutator:
    def __bool__(self):
        shared["tracked"] = 100
        return True


class IterMutator:
    def __iter__(self):
        shared["tracked"] = 110
        return iter(())


neg_mutator = NegMutator()
add_mutator = AddMutator()
attr_mutator = AttrMutator()
item_mutator = ItemMutator()
format_mutator = FormatMutator()
hash_mutator = HashMutator()
setitem_mutator = SetItemMutator()
truth_mutator = TruthMutator()
iter_mutator = IterMutator()

results = []

# Direct storage-boundary mutation through function.__globals__.
tracked = 10
shared["tracked"] = 20
results.append(tracked + 1)

# Unary and binary user protocols.
tracked = 10
-neg_mutator
results.append(tracked + 1)

tracked = 10
add_mutator + 0
results.append(tracked + 1)

# Attribute/item protocols.
tracked = 10
attr_mutator.trigger
results.append(tracked + 1)

tracked = 10
item_mutator[0]
results.append(tracked + 1)

# Formatting and dict-key hashing.
tracked = 10
f"{format_mutator}"
results.append(tracked + 1)

tracked = 10
{hash_mutator: "value"}
results.append(tracked + 1)

# Mutating item protocol.
tracked = 10
setitem_mutator[0] = 1
results.append(tracked + 1)

# Truthiness and iteration are already control-flow boundaries, but remain in
# the fixture so the shared effect policy is exercised end-to-end.
tracked = 10
if truth_mutator:
    pass
results.append(tracked + 1)

tracked = 10
for ignored in iter_mutator:
    raise AssertionError("empty iterator yielded")
results.append(tracked + 1)

assert results == [21, 31, 41, 51, 61, 71, 81, 91, 101, 111]
assert shared["tracked"] == 110
print("const-prop-namespace-reentry", results)
