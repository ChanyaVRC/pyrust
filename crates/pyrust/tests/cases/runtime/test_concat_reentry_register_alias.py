"""A fused + chain may re-enter Python without retaining register references."""


events = []
right = None


class Operand:
    def __init__(self, name):
        self.name = name

    def __add__(self, other):
        global right
        events.append((self.name, other.name))
        right = Operand("replacement")
        return Operand(self.name + other.name)


left = Operand("left")
middle = Operand("middle")
right = Operand("right")
result = left + middle + right
print(events)
print(result.name)
print(right.name)
