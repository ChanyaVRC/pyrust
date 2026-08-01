# Which operand's element object survives a set intersection (issue #2955).
#
# CPython scans the smaller table and inserts the *scanned* side's element, so
# among `__eq__`-equal-but-distinguishable elements (`1 == 1.0 == True`) the
# surviving representative comes from the smaller operand — the argument wins
# ties, and a non-set iterable argument is always the scanned side.


def t(s):
    return type(next(iter(s))).__name__


print("--- & operator ---")
print(t({1.0} & {1, 2, 3}))
print(t({1, 2, 3} & {1.0}))
print(t({1} & {1.0}))
print(t({1.0} & {1}))
print(t({True} & {1.0}))
print(t({1.0, 5, 6} & {True}))

print("--- intersection() with a set ---")
print(t({1.0}.intersection({1, 2, 3})))
print(t({1, 2, 3}.intersection({1.0})))
print(t({1}.intersection({1.0})))
print(t({1.0}.intersection({1})))
print(t({1.0}.intersection(frozenset({1}))))

print("--- intersection() with a non-set iterable ---")
print(t({1.0}.intersection([1, 2, 3])))
print(t({1, 2, 3}.intersection([1.0])))
print(t({1}.intersection([1.0])))
print(t({1}.intersection((1.0,))))
print(t({1}.intersection(iter([1.0]))))

print("--- multi-argument fold ---")
print(t({1}.intersection({1.0}, {True})))
print(t({1.0}.intersection({1}, {True})))
print(t({True}.intersection({1}, {1.0})))
print(t({1, 2, 3, 4}.intersection({1.0, 9}, {True, 8, 7})))
print(t({1}.intersection()))

print("--- intersection_update / &= ---")
s = {1.0}
s.intersection_update({1, 2, 3})
print(t(s))
s = {1, 2, 3}
s.intersection_update({1.0})
print(t(s))
s = {1}
s.intersection_update({1.0})
print(t(s))
s = {1.0}
s.intersection_update({1})
print(t(s))
s = {1}
s.intersection_update([1.0])
print(t(s))
s = {1, 2, 3}
s.intersection_update([1.0])
print(t(s))
s = {1}
s.intersection_update({1.0}, {True})
print(t(s))

s = {1.0}
s &= {1, 2, 3}
print(t(s))
s = {1, 2, 3}
s &= {1.0}
print(t(s))
s = {1}
s &= {1.0}
print(t(s))
s = {1.0}
s &= {1}
print(t(s))

print("--- frozenset ---")
print(t(frozenset({1.0}) & frozenset({1, 2, 3})))
print(t(frozenset({1, 2, 3}) & frozenset({1.0})))
print(t(frozenset({1}) & frozenset({1.0})))
print(t(frozenset({1.0}) & frozenset({1})))
print(t(frozenset({1.0}) & {1, 2, 3}))
print(t({1.0} & frozenset({1, 2, 3})))
print(t(frozenset({1}).intersection({1.0})))
print(t(frozenset({1}).intersection([1.0])))
print(t(frozenset({1, 2, 3}).intersection(frozenset({1.0}))))

print("--- nested tuple keys ---")
print({(1,)} & {(1.0,)})
print({(1.0,)} & {(1,)})
print({(1, 2), (3, 4)} & {(1.0, 2.0)})
print({(1,)}.intersection([(1.0,)]))


print("--- set subclass receiver ---")


class S(set):
    pass


print(t(S({1.0}) & {1, 2, 3}))
print(t(S({1, 2, 3}) & {1.0}))
print(t(S({1}).intersection({1.0})))
s = S({1, 2, 3})
s &= {1.0}
print(t(s))

print("--- user __eq__/__hash__ elements ---")


class K:
    def __init__(self, tag):
        self.tag = tag

    def __hash__(self):
        return 99

    def __eq__(self, other):
        return isinstance(other, K)

    def __repr__(self):
        return "K(%s)" % self.tag


a = K("a")
b = K("b")
c = K("c")
print(next(iter({a} & {b})))
print(next(iter({a, 1, 2} & {b})))
print(next(iter({a} & {b, 1, 2})))
print(next(iter({a}.intersection({b}))))
print(next(iter({a}.intersection([b]))))
print(next(iter({a, 1, 2}.intersection([b]))))
print(next(iter({a}.intersection({b}, {c}))))
s = {a}
s.intersection_update({b})
print(next(iter(s)))
s = {a, 1, 2}
s.intersection_update({b})
print(next(iter(s)))
s = {a}
s &= {b}
print(next(iter(s)))
s = {a, 1, 2}
s &= {b}
print(next(iter(s)))

print("--- __eq__ call count is unchanged ---")
calls = []


class C:
    def __init__(self, tag):
        self.tag = tag

    def __hash__(self):
        return 7

    def __eq__(self, other):
        calls.append((self.tag, other.tag))
        return True

    def __repr__(self):
        return "C(%s)" % self.tag


x = C("x")
y = C("y")
print(next(iter({x} & {y})), calls)

print("--- union / difference / symmetric_difference retention ---")
print(t({1} | {1.0}))
print(t({1.0} | {1}))
print(t({1}.union({1.0})))
print(t({1.0}.union([1])))
print(t({1.0} - {2}))
print(t({1.0}.difference([2])))
print(t({1.0} ^ {2, 1}))
print(t({1} ^ {2, 1.0}))

print("--- results and emptiness ---")
print(sorted({1, 2, 3} & {2, 3, 4}))
print(sorted({1, 2, 3}.intersection([2, 3, 4], (3, 5))))
print({1, 2} & {3, 4})
print(set() & {1})
print({1} & set())
print(set().intersection([1, 2]))
print({1, 2}.intersection([]))
s = {1, 2}
s.intersection_update(set())
print(s)
print(frozenset({1, 2}) & frozenset())
