# In-place set subtraction must be linear for primitive sets and must retain
# Python equality direction, partial-error, aliasing, and subclass semantics.


plain = set(range(2_000))
plain -= set(range(1_000, 3_000))
print("plain", len(plain), sum(plain))


class S(set):
    pass


sub = S(range(2_000))
sub -= set(range(1_000, 3_000))
print("subclass", type(sub).__name__, len(sub), sum(sub))


aliased = {1, 2, 3}
same = aliased
aliased -= same
print("self-alias", sorted(aliased), aliased is same)


events = []


class Stored:
    def __hash__(self):
        return 7

    def __eq__(self, other):
        events.append("stored-eq")
        return isinstance(other, Probe)


class Probe:
    def __hash__(self):
        return 7

    def __eq__(self, other):
        events.append("probe-eq")
        return False


left = {Stored()}
left -= {Probe()}
print("eq-direction", len(left), events)

sub_left = S({Stored()})
events.clear()
sub_left -= {Probe()}
print("subclass-eq", type(sub_left).__name__, len(sub_left), events)


class DifferenceBoom(Exception):
    pass


class Key:
    def __init__(self, name, hash_value):
        self.name = name
        self.hash_value = hash_value

    def __hash__(self):
        return self.hash_value

    def __eq__(self, other):
        if self.name == "b" and other.name == "boom":
            raise DifferenceBoom("eq")
        return self.hash_value == other.hash_value


a = Key("a", 1)
b = Key("b", 2)
left = {a, b}
right = {Key("aa", 1), Key("boom", 2)}
try:
    left -= right
except DifferenceBoom:
    print("eq-partial", sorted(item.name for item in left))
