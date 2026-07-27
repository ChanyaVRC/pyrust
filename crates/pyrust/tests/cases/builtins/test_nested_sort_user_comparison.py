"""Nested containers still dispatch comparison dunders during sorting."""


class Item:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        print("lt", self.value, other.value)
        return self.value < other.value

    def __repr__(self):
        return f"Item({self.value})"


class Prefix:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        print("eq-prefix", self.value, other.value)
        return self.value == other.value

    def __repr__(self):
        return f"Prefix({self.value})"


class ReflectedLeft:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        print("left-eq", self.value, other.value)
        return NotImplemented

    def __lt__(self, other):
        print("left-lt", self.value, other.value)
        return NotImplemented

    def __repr__(self):
        return f"ReflectedLeft({self.value})"


class ReflectedRight:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        print("right-eq", self.value, other.value)
        return False

    def __gt__(self, other):
        print("right-gt", self.value, other.value)
        return self.value > other.value

    def __repr__(self):
        return f"ReflectedRight({self.value})"


lists = [[Item(2)], [Item(1)]]
lists.sort()
print(lists)

tuples = sorted([(Item(3),), (Item(0),)])
print(tuples)

keyed = [Item(4), Item(1)]
keyed.sort(key=lambda item: [item])
print(keyed)

prefix = [[Prefix(7), Item(3)], [Prefix(7), Item(1)]]
prefix.sort()
print(prefix)

reflected = [[ReflectedRight(2)], [ReflectedLeft(1)]]
reflected.sort()
print(reflected)

maximum = max([(Item(2),), (Item(5),)])
print(maximum)
