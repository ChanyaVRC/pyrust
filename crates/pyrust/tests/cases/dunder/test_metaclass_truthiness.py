# A class object resolves truth-value slots on its metaclass.


events = []


class BoolMeta(type):
    def __bool__(cls):
        events.append(("bool", cls.__name__))
        return False


class BoolClass(metaclass=BoolMeta):
    pass


print(bool(BoolClass), not BoolClass, events)
events.clear()


class LenMeta(type):
    def __len__(cls):
        events.append(("len", cls.__name__))
        return 0


class LenClass(metaclass=LenMeta):
    pass


print(bool(LenClass), not LenClass, events)
events.clear()
print(len(LenClass), events)
events.clear()


class BothMeta(type):
    def __bool__(cls):
        events.append("bool-first")
        return True

    def __len__(cls):
        events.append("len-second")
        return 0


class BothClass(metaclass=BothMeta):
    pass


print(bool(BothClass), events)


class BadBoolMeta(type):
    def __bool__(cls):
        return 1


class BadBoolClass(metaclass=BadBoolMeta):
    pass


try:
    bool(BadBoolClass)
except Exception as exc:
    print(type(exc).__name__, str(exc))


class BadLenMeta(type):
    def __len__(cls):
        return -1


class BadLenClass(metaclass=BadLenMeta):
    pass


try:
    bool(BadLenClass)
except Exception as exc:
    print(type(exc).__name__, str(exc))


class HugeLen:
    def __len__(self):
        return 2**63


try:
    bool(HugeLen())
except Exception as exc:
    print(type(exc).__name__, str(exc))
