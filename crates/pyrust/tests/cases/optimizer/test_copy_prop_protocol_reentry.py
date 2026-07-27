# A Python protocol opcode is a namespace-mutation boundary even when it is not
# an explicit Call instruction.  __add__ writes through the live exec()
# namespace mirror here, so the later read of x must not be copy-propagated
# back to its pre-callback alias a.

namespace = {}


class Box:
    def __init__(self, tag):
        self.tag = tag


class Mutator:
    def __init__(self, namespace):
        self.namespace = namespace

    def __add__(self, other):
        self.namespace["x"] = Box("new")
        return 0


namespace["old"] = Box("old")
namespace["mutator"] = Mutator(namespace)
exec(
    "a = old\n"
    "x = a\n"
    "mutator + 0\n"
    "result = x.tag\n",
    namespace,
)

print(namespace["x"].tag)
print(namespace["result"])
