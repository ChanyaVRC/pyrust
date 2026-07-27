from collections import deque


class Mutating:
    def __init__(self, owner):
        self.owner = owner

    def __eq__(self, other):
        self.owner.append("changed")
        return False


def exercise(label, operation):
    owner = deque()
    owner.append(Mutating(owner))
    owner.append("tail")
    try:
        operation(owner)
    except Exception as error:
        print(label + ":", type(error).__name__, str(error), len(owner))


exercise("count", lambda owner: owner.count("target"))
exercise("index", lambda owner: owner.index("target"))
exercise("contains", lambda owner: "target" in owner)
exercise("remove", lambda owner: owner.remove("target"))


class EqualMutating(Mutating):
    def __eq__(self, other):
        self.owner.append("changed")
        return True


for operator in ("eq", "ne"):
    owner = deque()
    owner.append(EqualMutating(owner))
    other = deque(["target"])
    try:
        if operator == "eq":
            owner == other
        else:
            owner != other
    except RuntimeError as error:
        print(operator + ":", type(error).__name__, str(error), len(owner))
