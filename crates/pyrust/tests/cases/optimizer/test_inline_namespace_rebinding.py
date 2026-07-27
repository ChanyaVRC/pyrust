namespace = {}


def replacement(value):
    return value + 100


class Trigger:
    def go(self):
        namespace["helper"] = replacement


namespace["trigger"] = Trigger()
exec(
    "def helper(value):\n"
    "    return value + 1\n"
    "trigger.go()\n"
    "result = helper(2)\n",
    namespace,
)

print("runtime function rebinding:", namespace["result"])
