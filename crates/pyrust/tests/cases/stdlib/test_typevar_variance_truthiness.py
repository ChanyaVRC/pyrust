# TypeVar variance keyword values are converted with Python's truth protocol,
# rather than being treated as true merely because they are instances.

from typing import TypeVar

events = []


class Flag:
    def __init__(self, label, answer):
        self.label = label
        self.answer = answer

    def __bool__(self):
        events.append(self.label)
        return self.answer


T = TypeVar(
    "T",
    covariant=Flag("covariant", False),
    contravariant=Flag("contravariant", True),
)
print("variance", repr(T), T.__covariant__, T.__contravariant__)

I = TypeVar("I", infer_variance=Flag("infer_variance", True))
print("inferred", repr(I), I.__infer_variance__)
print("events", events)


class Boom:
    def __bool__(self):
        raise RuntimeError("variance truth boom")


try:
    TypeVar("B", covariant=Boom())
except Exception as exc:
    print("error", type(exc).__name__, str(exc))
else:
    print("error missing")
