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


caller_fastlocal_events = []


class CallerFastlocalTracked:
    def __del__(self):
        caller_fastlocal_events.append("finalized")


def delete_with_caller_fastlocal_alias():
    alias = CallerFastlocalTracked()

    def owner(seed):
        value = seed
        del seed

        def delete_value():
            nonlocal value
            del value

        delete_value()

    owner(alias)
    print("caller fastlocal after nonlocal delete:", caller_fastlocal_events)
    del alias
    print("caller fastlocal after alias drop:", caller_fastlocal_events)


delete_with_caller_fastlocal_alias()


caller_cell_events = []


class CallerCellTracked:
    def __del__(self):
        caller_cell_events.append("finalized")


def delete_with_caller_cell_alias():
    alias = CallerCellTracked()

    def owner():
        value = alias

        def delete_value():
            nonlocal value
            del value

        delete_value()

    owner()
    print("caller cell after nonlocal delete:", caller_cell_events)
    del alias
    print("caller cell after alias drop:", caller_cell_events)


delete_with_caller_cell_alias()


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


generator_owner_events = []


class GeneratorOwnerTracked:
    def __del__(self):
        generator_owner_events.append("finalized")


def generator_owner():
    value = GeneratorOwnerTracked()
    alias = value

    def delete_value():
        nonlocal value
        del value

    delete_value()
    print("generator owner after nonlocal delete:", generator_owner_events)
    del alias
    print("generator owner after alias drop:", generator_owner_events)
    yield None


list(generator_owner())


coroutine_owner_events = []


class CoroutineOwnerTracked:
    def __del__(self):
        coroutine_owner_events.append("finalized")


async def coroutine_owner():
    value = CoroutineOwnerTracked()
    alias = value

    def delete_value():
        nonlocal value
        del value

    delete_value()
    print("coroutine owner after nonlocal delete:", coroutine_owner_events)
    del alias
    print("coroutine owner after alias drop:", coroutine_owner_events)


coroutine = coroutine_owner()
try:
    coroutine.send(None)
except StopIteration:
    pass


suspended_generator_events = []


class SuspendedGeneratorTracked:
    def __del__(self):
        suspended_generator_events.append("finalized")


def suspended_generator_owner():
    value = SuspendedGeneratorTracked()
    alias = value

    def delete_value():
        nonlocal value
        del value

    yield delete_value
    print("suspended generator after resume:", suspended_generator_events)
    del alias
    print("suspended generator after alias drop:", suspended_generator_events)


suspended_generator = suspended_generator_owner()
delete_suspended_generator_value = next(suspended_generator)
delete_suspended_generator_value()
print("suspended generator after nonlocal delete:", suspended_generator_events)
try:
    next(suspended_generator)
except StopIteration:
    pass


suspended_coroutine_events = []
suspended_coroutine_deleters = []


class SuspendedCoroutineTracked:
    def __del__(self):
        suspended_coroutine_events.append("finalized")


class SuspendCoroutine:
    def __await__(self):
        yield None


async def suspended_coroutine_owner():
    value = SuspendedCoroutineTracked()
    alias = value

    def delete_value():
        nonlocal value
        del value

    suspended_coroutine_deleters.append(delete_value)
    await SuspendCoroutine()
    print("suspended coroutine after resume:", suspended_coroutine_events)
    del alias
    print("suspended coroutine after alias drop:", suspended_coroutine_events)


suspended_coroutine = suspended_coroutine_owner()
suspended_coroutine.send(None)
delete_suspended_coroutine_value = suspended_coroutine_deleters.pop()
delete_suspended_coroutine_value()
print("suspended coroutine after nonlocal delete:", suspended_coroutine_events)
try:
    suspended_coroutine.send(None)
except StopIteration:
    pass
