# PEP 634 sequence-pattern type exclusion (issue #1789).
#
# str, bytes, dict, set, and frozenset are NOT matched by sequence patterns,
# even though they support len().  list/tuple/range and list/tuple subclasses
# ARE matched.  The compiler lowers this exclusion to a single
# `MatchSeqExcluded` instruction; this fixture pins the observable behaviour
# (and its subclass handling) byte-for-byte against CPython.


def classify(x):
    match x:
        case [a, b]:
            return ("seq2", a, b)
        case [a]:
            return ("seq1", a)
        case [a, *rest]:
            return ("seqstar", a, rest)
        case []:
            return "empty"
        case _:
            return "other"


# Matched: real sequences.
print(classify([1, 2]))
print(classify((1, 2)))
print(classify([1]))
print(classify([1, 2, 3]))
print(classify([]))
print(classify(range(2)))

# Excluded: str / bytes / dict / set / frozenset.
print(classify("ab"))
print(classify("a"))
print(classify(b"ab"))
print(classify({1: 10, 2: 20}))
print(classify({1, 2}))
print(classify(frozenset({1, 2})))
print(classify(""))
print(classify({}))
print(classify(set()))


# Subclasses follow isinstance: list/tuple subclasses match, the excluded
# types' subclasses do not.
class MyList(list):
    pass


class MyTuple(tuple):
    pass


class MyDict(dict):
    pass


class MyStr(str):
    pass


class MySet(set):
    pass


class MyFrozen(frozenset):
    pass


print(classify(MyList([1, 2])))
print(classify(MyTuple((1, 2))))
print(classify(MyDict({1: 2, 3: 4})))
print(classify(MyStr("ab")))
print(classify(MySet({1, 2})))
print(classify(MyFrozen({1, 2})))


# Exclusion still applies inside nested patterns.
def nested(x):
    match x:
        case [[a, b], c]:
            return (a, b, c)
        case _:
            return None


print(nested([[1, 2], 3]))
print(nested([["ab", 9], 3]))  # inner str fails the inner [a, b]
print(nested([{1: 2}, 3]))     # inner dict fails the inner [a, b]


# Exclusion still applies inside an OR pattern with a sequence alternative.
def or_seq(x):
    match x:
        case [_, _] | (1, 2, 3):
            return "matched"
        case _:
            return "nope"


print(or_seq([1, 2]))
print(or_seq((1, 2, 3)))
print(or_seq("xy"))      # str: excluded from [_, _]
print(or_seq({1: 2}))    # dict: excluded


# A match in a tight loop must stay correct across many iterations.
total = 0
for i in range(1000):
    match [i, i + 1]:
        case [p, q]:
            total += q - p
print(total)
