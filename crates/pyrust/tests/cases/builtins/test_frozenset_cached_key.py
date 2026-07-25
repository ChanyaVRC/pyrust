# Equal frozensets with different insertion orders remain interchangeable keys.
a = frozenset([3, 1, 2])
b = frozenset([2, 3, 1])
print(a == b, hash(a) == hash(b))
print(frozenset(a) is a)

d = {a: "first"}
d[b] = "second"
print(len(d), d[a], d[b])

# Converting stored PyKey values back through iteration/pop/set operations keeps
# the frozenset value and its nested contents intact.
stored = next(iter(d))
print(isinstance(stored, frozenset), stored == a, d[stored])
key, value = d.popitem()
print(key == b, value, len(d))

s = {a}
print(b in s)
s.remove(b)
print(len(s))

nested_a = frozenset([a, frozenset([8, 9])])
nested_b = frozenset([frozenset([9, 8]), b])
mapping = {nested_a: 42}
print(nested_a == nested_b, hash(nested_a) == hash(nested_b), mapping[nested_b])

# Nested user objects still go through interpreter-aware equality.
class Key:
    def __init__(self, value):
        self.value = value

    def __hash__(self):
        return 12345

    def __eq__(self, other):
        return isinstance(other, Key) and self.value == other.value

left = frozenset([Key(7)])
right = frozenset([Key(7)])
objects = {left: "left"}
objects[right] = "right"
print(len(objects), objects[left], objects[right])
