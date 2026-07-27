# Rich-comparison slots may return any object.  The comparison expression
# itself preserves that object, while operations that need a boolean result
# must run the normal __bool__ / __len__ protocol.

events = []


class BoolVerdict:
    def __init__(self, label, value):
        self.label = label
        self.value = value

    def __bool__(self):
        events.append("bool:" + self.label)
        return self.value


class LenVerdict:
    def __init__(self, label, value):
        self.label = label
        self.value = value

    def __len__(self):
        events.append("len:" + self.label)
        return self.value


class EqResult:
    def __init__(self, label, verdict):
        self.label = label
        self.verdict = verdict

    def __eq__(self, other):
        events.append("eq:" + self.label)
        return self.verdict


# A direct == returns the rich-comparison result without coercing it.
direct_verdict = BoolVerdict("direct", False)
direct_result = EqResult("direct", direct_verdict) == object()
print(direct_result is direct_verdict)
print(events)
events.clear()

# Inherited object.__ne__ negates __eq__ via the full truthiness protocol.
ne_verdict = BoolVerdict("ne", False)
print(EqResult("ne", ne_verdict) != object())
print(events)
events.clear()


class DeferringNe:
    def __ne__(self, other):
        events.append("defer-ne")
        return NotImplemented


# The reflected inherited object.__ne__ step has the same coercion rule.
reflected_verdict = BoolVerdict("reflected-ne", False)
print(DeferringNe() != EqResult("reflected-ne", reflected_verdict))
print(events)
events.clear()

# Sequence and mapping equality also coerce an element/value result via
# __bool__, rather than inspecting the result's raw storage.
list_verdict = BoolVerdict("list", False)
print([EqResult("list", list_verdict)] == [object()])
print(events)
events.clear()

tuple_verdict = LenVerdict("tuple", 0)
print((EqResult("tuple", tuple_verdict),) == (object(),))
print(events)
events.clear()

dict_verdict = BoolVerdict("dict", False)
print({1: EqResult("dict", dict_verdict)} == {1: object()})
print(events)
events.clear()


class ExplicitNe:
    def __init__(self, verdict):
        self.verdict = verdict

    def __ne__(self, other):
        events.append("explicit-ne")
        return self.verdict


# An explicit __ne__ result remains uncoerced.
explicit_verdict = BoolVerdict("explicit", False)
explicit_result = ExplicitNe(explicit_verdict) != object()
print(explicit_result is explicit_verdict)
print(events)
events.clear()


class BadVerdict:
    def __bool__(self):
        return 1


try:
    [EqResult("bad", BadVerdict())] == [object()]
except Exception as exc:
    print(type(exc).__name__)
print(events)
