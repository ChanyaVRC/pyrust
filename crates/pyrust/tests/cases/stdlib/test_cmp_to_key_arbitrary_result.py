# functools.cmp_to_key rich comparisons return the arbitrary object produced
# by comparing cmp(a, b) with zero.  They do not coerce it to bool.

from functools import cmp_to_key

events = []


class Marker:
    def __init__(self, label):
        self.label = label

    def __bool__(self):
        events.append("unexpected-marker-bool:" + self.label)
        raise AssertionError("cmp_to_key result was coerced to bool")


class Comparison:
    def result(self, op):
        events.append("comparison:" + op)
        return Marker(op)

    def __lt__(self, other):
        return self.result("lt")

    def __le__(self, other):
        return self.result("le")

    def __eq__(self, other):
        return self.result("eq")

    def __ne__(self, other):
        return self.result("ne")

    def __gt__(self, other):
        return self.result("gt")

    def __ge__(self, other):
        return self.result("ge")


def compare(left, right):
    events.append("cmp:" + left + ":" + right)
    return Comparison()


key = cmp_to_key(compare)
left = key("left")
right = key("right")


def describe(value):
    return type(value).__name__ + ":" + getattr(value, "label", "-")


print("lt", describe(left < right))
print("le", describe(left <= right))
print("eq", describe(left == right))
print("ne", describe(left != right))
print("gt", describe(left > right))
print("ge", describe(left >= right))
print("events", events)
