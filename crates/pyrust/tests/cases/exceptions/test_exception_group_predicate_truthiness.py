# ExceptionGroup.subgroup()/split() predicates may return arbitrary objects.
# Each result must be converted through __bool__, including errors it raises.

events = []


class Decision:
    def __init__(self, label, answer):
        self.label = label
        self.answer = answer

    def __bool__(self):
        events.append("bool:" + self.label)
        return self.answer


group = ExceptionGroup("root", [ValueError("v"), TypeError("t")])


def reject_everything(exc):
    label = type(exc).__name__
    events.append("call:" + label)
    return Decision(label, False)


print("subgroup-is-none", group.subgroup(reject_everything) is None)
print("events", events)


class Boom:
    def __bool__(self):
        raise RuntimeError("predicate truth boom")


try:
    group.split(lambda exc: Boom())
except Exception as exc:
    print("error", type(exc).__name__, str(exc))
else:
    print("error missing")
