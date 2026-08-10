# Issue #3031: deleting a nonlocal clears the enclosing function's cell.


def outer():
    value = "bound"

    def delete_value():
        nonlocal value
        del value

    def read_value():
        return value

    try:
        delete_value()
        print("nonlocal delete: ok")
    except Exception as exc:
        print("nonlocal delete:", type(exc).__name__)

    try:
        read_value()
    except Exception as exc:
        print("free read:", type(exc).__name__)

    try:
        value
    except Exception as exc:
        print("owner read:", type(exc).__name__)


outer()


finalizer_events = []


class Tracked:
    def __del__(self):
        finalizer_events.append("finalized")


def delete_with_finalizer():
    value = Tracked()

    def delete_value():
        nonlocal value
        del value

    print("finalizer before:", finalizer_events)
    delete_value()
    print("finalizer after:", finalizer_events)


delete_with_finalizer()


live_alias_events = []


class LiveAliasTracked:
    def __del__(self):
        live_alias_events.append("finalized")


def delete_with_live_outer_alias():
    value = LiveAliasTracked()
    alias = value

    def delete_value():
        nonlocal value
        del value

    delete_value()
    print("live alias after nonlocal delete:", live_alias_events)
    del alias
    print("live alias after alias drop:", live_alias_events)


delete_with_live_outer_alias()


inner_alias_events = []


class InnerAliasTracked:
    def __del__(self):
        inner_alias_events.append("finalized")


def delete_with_inner_alias():
    value = InnerAliasTracked()

    def delete_value(alias=value):
        nonlocal value
        del value
        print(
            "inner alias after nonlocal delete:",
            inner_alias_events,
            alias is not None,
        )

    delete_value()


delete_with_inner_alias()


closed_alias_events = []


class ClosedAliasTracked:
    def __del__(self):
        closed_alias_events.append("finalized")


def make_closed_alias_deleters():
    value = ClosedAliasTracked()
    alias = value

    def delete_value():
        nonlocal value
        del value

    def delete_alias():
        nonlocal alias
        del alias

    return delete_value, delete_alias


delete_closed_value, delete_closed_alias = make_closed_alias_deleters()
delete_closed_value()
print("closed alias after nonlocal delete:", closed_alias_events)
delete_closed_alias()
print("closed alias after alias drop:", closed_alias_events)


rebind_events = []


class RebindTracked:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        rebind_events.append(self.label)


def delete_rebind_and_delete_again():
    value = RebindTracked("old")
    alias = value

    def mutate_value():
        nonlocal value
        del value
        value = RebindTracked("new")
        del value
        try:
            del value
        except Exception as exc:
            return type(exc).__name__

    second_delete_error = mutate_value()
    print("rebind after nonlocal deletes:", rebind_events, second_delete_error)
    del alias
    print("rebind after alias drop:", rebind_events)


delete_rebind_and_delete_again()
