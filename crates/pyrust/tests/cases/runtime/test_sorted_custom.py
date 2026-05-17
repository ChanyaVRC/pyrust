class Item:
    def __init__(self, v):
        self.v = v
    def __lt__(self, other):
        return self.v < other.v
    def __le__(self, other):
        return self.v <= other.v
    def __gt__(self, other):
        return self.v > other.v
    def __ge__(self, other):
        return self.v >= other.v
    def __repr__(self):
        return f'Item({self.v})'

items = [Item(3), Item(1), Item(2)]
result = sorted(items)
print([x.v for x in result])   # [1, 2, 3]

print(min(Item(3), Item(1)).v)  # 1
print(max(Item(3), Item(1)).v)  # 3

# sorted with key=
words = ['banana', 'apple', 'cherry']
print(sorted(words, key=len))   # ['apple', 'banana', 'cherry']

# reverse=True
print([x.v for x in sorted(items, reverse=True)])  # [3, 2, 1]

# min/max with equal elements returns first encountered
a, b = Item(5), Item(5)
assert min(a, b) is a, "min of equal items should return first"
assert max(a, b) is a, "max of equal items should return first"

# min/max on primitives still works
print(min(3, 1, 2))   # 1
print(max(3, 1, 2))   # 3
print(min([5, 2, 8]))  # 2
print(max([5, 2, 8]))  # 8
print("ok")
