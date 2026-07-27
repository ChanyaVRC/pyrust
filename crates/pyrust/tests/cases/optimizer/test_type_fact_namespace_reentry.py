# Optimizer type facts may outlive bytecode writes only for temporary registers.
# A named register can be replaced through a live explicit namespace while a
# protocol method is running, so a prior LoadConst is not a durable type proof.


def run_case(source):
    namespace = {}
    exec(source, namespace, namespace)
    return namespace


const_reg = run_case(
    """
events = []

class Mutator:
    def __add__(self, other):
        events.append("rhs")
        globals()["rhs"] = 5
        return 0

rhs = 0
Mutator() + 0
result = 10 + rhs
"""
)
print("const-reg", const_reg["result"], const_reg["events"])


algebraic = run_case(
    """
events = []

class Replacement:
    def __add__(self, other):
        events.append(("replacement", other))
        return "replacement-result"

class Mutator:
    def __add__(self, other):
        globals()["lhs"] = Replacement()
        return 0

lhs = 1
Mutator() + 0
result = lhs + 0
"""
)
print("algebraic", algebraic["result"], algebraic["events"])


reassoc = run_case(
    """
events = []

class First:
    def __add__(self, other):
        events.append(("first", other))
        return Second()

class Second:
    def __add__(self, other):
        events.append(("second", other))
        return "chain-result"

class Mutator:
    def __add__(self, other):
        globals()["chain"] = First()
        return 0

chain = 1
Mutator() + 0
result = (chain + 40000) + 50000
"""
)
print("reassoc", reassoc["result"], reassoc["events"])
