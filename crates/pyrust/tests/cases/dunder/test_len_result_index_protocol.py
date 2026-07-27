# `len()` and truth-value testing share the same validation of `__len__`
# results, including the full `__index__` protocol.


def outcome(operation, value):
    try:
        return ("ok", operation(value))
    except Exception as exc:
        return (type(exc).__name__, str(exc))


events = []


class IndexResult:
    def __init__(self, value, label):
        self.value = value
        self.label = label

    def __index__(self):
        events.append(self.label)
        return self.value


class BadIndexResult:
    def __index__(self):
        events.append("bad-index")
        return 1.5


class FloatSubclass(float):
    pass


class BadSubclassIndexResult:
    def __index__(self):
        events.append("bad-subclass-index")
        return FloatSubclass(1.5)


class NoIndexResult:
    pass


class Length:
    def __init__(self, result):
        self.result = result

    def __len__(self):
        return self.result


for label, result in (
    ("zero", IndexResult(0, "zero")),
    ("positive", IndexResult(3, "positive")),
    ("negative", IndexResult(-1, "negative")),
    ("huge", IndexResult(2**63, "huge")),
    ("huge-negative", IndexResult(-(2**80), "huge-negative")),
    ("bad-index", BadIndexResult()),
    ("bad-subclass-index", BadSubclassIndexResult()),
    ("no-index", NoIndexResult()),
):
    value = Length(result)
    print(label, outcome(len, value), outcome(bool, value))


class IntSubclass(int):
    def __index__(self):
        events.append("int-subclass-override")
        return 99


for value in (IntSubclass(0), IntSubclass(4), IntSubclass(-1)):
    wrapped = Length(value)
    print("int-subclass", int(value), outcome(len, wrapped), outcome(bool, wrapped))


index_to_subclass = Length(IndexResult(IntSubclass(5), "index-int-subclass"))
print(
    "index-int-subclass",
    outcome(len, index_to_subclass),
    outcome(bool, index_to_subclass),
)


class LengthMeta(type):
    def __len__(cls):
        return IndexResult(cls.length, "meta-" + cls.__name__)


class EmptyClass(metaclass=LengthMeta):
    length = 0


class NonEmptyClass(metaclass=LengthMeta):
    length = 2


print("class-empty", outcome(len, EmptyClass), outcome(bool, EmptyClass))
print("class-nonempty", outcome(len, NonEmptyClass), outcome(bool, NonEmptyClass))
print("events", events)
