# sorted() must be stable in both directions: equal elements keep their
# input order regardless of reverse=. Regression test for #1904, where
# reverse=True was implemented by reversing the whole result (flipping
# equal runs) instead of inverting the comparison in the stable sort.

data = [(1, 'a'), (2, 'b'), (1, 'c'), (2, 'd'), (1, 'e')]

# key + reverse: equal keys keep input order
print(sorted(data, key=lambda x: x[0], reverse=True))
print(sorted(data, key=lambda x: x[0]))
print(sorted([(1, 'x'), (1, 'y'), (1, 'z')], key=lambda t: t[0], reverse=True))

# no key, duplicate primitive values
print(sorted([3, 1, 2, 1, 3, 2], reverse=True))
print(sorted([3, 1, 2, 1, 3, 2]))

# strings, with duplicates
print(sorted(['banana', 'apple', 'cherry', 'apple'], reverse=True))

# already-sorted / reverse-sorted input
print(sorted([1, 2, 3, 4], reverse=True))
print(sorted([4, 3, 2, 1], reverse=True))

# single / empty
print(sorted([], reverse=True))
print(sorted([5], reverse=True))

# stable reverse differs from slice-reversed ascending sort
x = [(1, 'a'), (2, 'b'), (1, 'c')]
print(sorted(x, key=lambda t: t[0], reverse=True))
print(sorted(x, key=lambda t: t[0])[::-1])

# sorted() and list.sort() must agree
l = list(data)
l.sort(key=lambda v: v[0], reverse=True)
print(l == sorted(data, key=lambda v: v[0], reverse=True))


# user class ordered only on .k — stability observed via .tag
class C:
    def __init__(self, k, tag):
        self.k = k
        self.tag = tag

    def __lt__(self, o):
        return self.k < o.k

    def __repr__(self):
        return f"C({self.k},{self.tag})"


items = [C(1, 'a'), C(2, 'b'), C(1, 'c'), C(2, 'd')]
print(sorted(items, reverse=True))
print(sorted(items))

# key function returning a user instance
print(sorted([('a', 1), ('b', 1), ('c', 2)], key=lambda t: C(t[1], t[0]), reverse=True))
