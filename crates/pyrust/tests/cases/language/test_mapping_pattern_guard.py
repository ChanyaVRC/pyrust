# PEP 634 mapping-pattern type guard (issue #1879).
#
# A mapping pattern (`case {k: p}`) matches only when the subject is a mapping
# (`isinstance(subject, collections.abc.Mapping)`).  For any non-mapping subject
# the pattern silently fails to match instead of raising on the per-key `key in
# subject` membership test.  The compiler lowers this guard to a single
# `MatchMapping` instruction (mirroring `MatchSeqExcluded` for sequence
# patterns); this fixture pins the observable behaviour byte-for-byte against
# CPython 3.12.


def classify(x):
    match x:
        case {0: v}:
            return ("k0", v)
        case _:
            return "other"


# Matched: real dict with the key.
print(classify({0: "z"}))
print(classify({0: 99}))

# Non-mappings must NOT match and must NOT raise (the original crash).
print(classify(5))
print(classify(3.5))
print(classify("ab"))
print(classify(b"ab"))
print(classify([10, 20]))
print(classify((1, 2)))
print(classify({1, 2}))
print(classify(frozenset({1, 2})))
print(classify(None))
print(classify(True))
print(classify(False))

# dict that simply lacks the key: a mapping, but no match.
print(classify({1: "nokey"}))


# An empty mapping pattern matches ANY mapping (including the empty dict) and
# nothing else.
def is_mapping(x):
    match x:
        case {}:
            return "mapping"
        case _:
            return "other"


print(is_mapping({}))
print(is_mapping({1: 2}))
print(is_mapping(5))
print(is_mapping("ab"))
print(is_mapping([]))
print(is_mapping(set()))


# A custom class that supports `in` / `[]` but is NOT a Mapping must not match.
class NotMap:
    def __contains__(self, k):
        return True

    def __getitem__(self, k):
        return "x"


print(classify(NotMap()))


# dict subclasses ARE mappings.
class MyDict(dict):
    pass


print(classify(MyDict({0: "sub"})))
print(is_mapping(MyDict()))


# A mappingproxy (`type(C).__dict__`) is registered as collections.abc.Mapping
# in CPython, so it matches a mapping pattern (key bind and empty pattern).
class WithAttr:
    a = 10
    b = 20


print(is_mapping(WithAttr.__dict__))


def proxy_bind(x):
    match x:
        case {"a": v}:
            return ("a", v)
        case _:
            return "other"


print(proxy_bind(WithAttr.__dict__))


# **rest capture still works on real mappings.
def rest(x):
    match x:
        case {0: v, **r}:
            return (v, r)
        case _:
            return "other"


print(rest({0: "z", 1: "a", 2: "b"}))
print(rest({0: "z"}))
print(rest(MyDict({0: 1, 9: 2})))
print(rest([1, 2]))
print(rest(5))


# Nested mapping patterns: inner non-mapping fails the inner pattern.
def nested(x):
    match x:
        case {"a": {"b": v}}:
            return v
        case _:
            return "other"


print(nested({"a": {"b": 7}}))
print(nested({"a": 5}))
print(nested({"a": [1]}))
print(nested(5))


# Mapping pattern with a guard.
def guarded(x):
    match x:
        case {0: v} if v > 10:
            return ("big", v)
        case {0: v}:
            return ("small", v)
        case _:
            return "other"


print(guarded({0: 99}))
print(guarded({0: 1}))
print(guarded(5))
print(guarded([1]))


# OR pattern mixing a mapping alternative with a sequence alternative.
def or_mix(x):
    match x:
        case {0: _} | [1, 2]:
            return "matched"
        case _:
            return "nope"


print(or_mix({0: "z"}))
print(or_mix([1, 2]))
print(or_mix("xy"))
print(or_mix(5))


# A match in a tight loop must stay correct across many iterations.
total = 0
for i in range(1000):
    match {0: i}:
        case {0: v}:
            total += v
print(total)
