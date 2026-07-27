"""Fused comparison jumps must run the result's truth-value protocol."""

events = []


class BoolVerdict:
    def __init__(self, label):
        self.label = label

    def __bool__(self):
        events.append(("bool", self.label))
        return False


class BoolOperand:
    def __init__(self, label):
        self.label = label

    def __add__(self, other):
        events.append(("add", self.label))
        return BoolVerdict(self.label)


class LenVerdict:
    def __len__(self):
        events.append(("len", "const-len"))
        return 0


class LenOperand:
    def __add__(self, other):
        events.append(("add", "const-len"))
        return LenVerdict()


def reg_false(left, right):
    if left + right:
        return "taken"
    return "skipped"


def reg_inverted_false(left, right):
    if not (left + right):
        return "taken"
    return "skipped"


def const_false(left):
    if left + 1:
        return "taken"
    return "skipped"


def const_inverted_false(left):
    if not (left + 1):
        return "taken"
    return "skipped"


results = [
    reg_false(BoolOperand("reg-false"), object()),
    reg_inverted_false(BoolOperand("reg-inverted"), object()),
    const_false(BoolOperand("const-false")),
    const_inverted_false(BoolOperand("const-inverted")),
    const_false(LenOperand()),
]

print(results)
print(events)
assert results == ["skipped", "taken", "skipped", "taken", "skipped"]
assert events == [
    ("add", "reg-false"),
    ("bool", "reg-false"),
    ("add", "reg-inverted"),
    ("bool", "reg-inverted"),
    ("add", "const-false"),
    ("bool", "const-false"),
    ("add", "const-inverted"),
    ("bool", "const-inverted"),
    ("add", "const-len"),
    ("len", "const-len"),
]
