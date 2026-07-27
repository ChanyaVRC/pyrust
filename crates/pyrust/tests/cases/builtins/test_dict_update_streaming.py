# Streaming dict.update(iterable-of-pairs) regression coverage.
#
# The iterable form must commit each pair as it is produced, while preserving
# normal dict key semantics for custom __hash__/__eq__ implementations.


class UpdateBoom(Exception):
    pass


def raising_pairs():
    yield ("first", 1)
    yield ("second", 2)
    raise UpdateBoom("stop")


d = {"before": 0}
try:
    d.update(raising_pairs())
except UpdateBoom:
    print("iteration-partial", sorted(d.items()))


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


stored = Stored()
d = {stored: "old"}
d.update(iter([(Probe(), "new")]))
only_key = next(iter(d))
print("stored-key", len(d), only_key is stored, d[stored], events)


class RaisingStored:
    def __hash__(self):
        return 11

    def __eq__(self, other):
        raise UpdateBoom("eq")


class RaisingProbe:
    def __hash__(self):
        return 11


d = {RaisingStored(): "kept"}
try:
    d.update(iter([("committed", 1), (RaisingProbe(), 2)]))
except UpdateBoom:
    print("eq-partial", d["committed"], len(d))
