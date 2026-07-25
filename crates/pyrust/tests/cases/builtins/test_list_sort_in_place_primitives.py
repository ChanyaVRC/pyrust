ints = [5, -1, 3, 3, 0, 9]
ints_alias = ints
print(ints.sort(), ints, ints_alias is ints)
ints.sort(reverse=True)
print(ints, ints_alias)

words = ["pear", "apple", "banana", "apple", ""]
words_alias = words
print(words.sort(), words, words_alias is words)
words.sort(reverse=True)
print(words, words_alias)

empty = []
print(empty.sort(), empty)


class NeverCompared:
    def __lt__(self, other):
        raise AssertionError("a one-element list must not compare")


single = [NeverCompared()]
print(single.sort(), len(single))


# Non-primitive lists retain the interpreter-aware comparator path.
class Item:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        return self.value < other.value


objects = [Item(3), Item(1), Item(2)]
objects.sort()
print([item.value for item in objects])
