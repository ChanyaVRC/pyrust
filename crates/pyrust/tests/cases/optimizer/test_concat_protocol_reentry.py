"""Concat fusion must preserve left-to-right namespace reads."""

namespace = {}
events = []


class Operand:
    def __init__(self, name):
        self.name = name

    def __add__(self, other):
        events.append(("add", self.name, other.name))
        return Operand(self.name + other.name)


class Middle:
    def __radd__(self, other):
        events.append(("radd", other))
        namespace["right"] = Operand("replacement")
        return Operand(other + "middle")


namespace["middle_obj"] = Middle()
namespace["right_obj"] = Operand("right")
exec(
    'left = "left"\n'
    "middle = middle_obj\n"
    "right = right_obj\n"
    "result = left + middle + right\n",
    namespace,
)

print(events)
print(namespace["result"].name)
assert events == [
    ("radd", "left"),
    ("add", "leftmiddle", "replacement"),
]
assert namespace["result"].name == "leftmiddlereplacement"
