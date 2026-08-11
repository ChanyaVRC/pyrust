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


shared_generator_events = []


class SharedGeneratorTracked:
    def __del__(self):
        shared_generator_events.append("finalized")


def delete_with_shared_generator_alias():
    value = SharedGeneratorTracked()

    def holder():
        alias = value
        yield None
        print("shared generator after resume:", shared_generator_events)
        del alias
        print("shared generator after alias drop:", shared_generator_events)

    def delete_value():
        nonlocal value
        del value

    generator = holder()
    next(generator)
    delete_value()
    print("shared generator after nonlocal delete:", shared_generator_events)
    try:
        next(generator)
    except StopIteration:
        pass


delete_with_shared_generator_alias()


shared_coroutine_events = []


class SharedCoroutineTracked:
    def __del__(self):
        shared_coroutine_events.append("finalized")


def delete_with_shared_coroutine_alias():
    value = SharedCoroutineTracked()

    async def holder():
        alias = value
        await SuspendCoroutine()
        print("shared coroutine after resume:", shared_coroutine_events)
        del alias
        print("shared coroutine after alias drop:", shared_coroutine_events)

    def delete_value():
        nonlocal value
        del value

    coroutine = holder()
    coroutine.send(None)
    delete_value()
    print("shared coroutine after nonlocal delete:", shared_coroutine_events)
    try:
        coroutine.send(None)
    except StopIteration:
        pass


delete_with_shared_coroutine_alias()


shared_multi_owner_events = []


class SharedMultiOwnerTracked:
    def __del__(self):
        shared_multi_owner_events.append("finalized")


def delete_with_multiple_shared_holders():
    value = SharedMultiOwnerTracked()

    def holder(keep_alias):
        alias = value if keep_alias else None
        yield None
        if alias is not None:
            del alias

    def delete_value():
        nonlocal value
        del value

    holding_generator = holder(True)
    empty_generator = holder(False)
    next(holding_generator)
    next(empty_generator)
    delete_value()
    print("multiple shared holders after nonlocal delete:", shared_multi_owner_events)
    try:
        next(holding_generator)
    except StopIteration:
        pass
    print("multiple shared holders after alias drop:", shared_multi_owner_events)
    try:
        next(empty_generator)
    except StopIteration:
        pass


delete_with_multiple_shared_holders()


done_holder_events = []


class DoneHolderTracked:
    def __del__(self):
        done_holder_events.append("finalized")


def delete_after_holder_finishes():
    value = DoneHolderTracked()

    def holder():
        alias = value
        yield None
        return alias is not None

    def delete_value():
        nonlocal value
        del value

    generator = holder()
    next(generator)
    try:
        next(generator)
    except StopIteration:
        pass
    delete_value()
    print("done shared holder after nonlocal delete:", done_holder_events)


delete_after_holder_finishes()


dropped_holder_events = []


class DroppedHolderTracked:
    def __del__(self):
        dropped_holder_events.append("finalized")


def delete_after_holder_is_dropped():
    value = DroppedHolderTracked()

    def holder():
        alias = value
        yield None
        return alias is not None

    def delete_value():
        nonlocal value
        del value

    generator = holder()
    next(generator)
    del generator
    delete_value()
    print("dropped shared holder after nonlocal delete:", dropped_holder_events)


delete_after_holder_is_dropped()


expired_holder_events = []


class ExpiredHolderTracked:
    def __del__(self):
        expired_holder_events.append("finalized")


def make_deleter_after_holder_expires():
    value = ExpiredHolderTracked()

    def holder():
        alias = value
        yield None
        return alias is not None

    def delete_value():
        nonlocal value
        del value

    generator = holder()
    next(generator)
    return delete_value


delete_expired_holder_value = make_deleter_after_holder_expires()
delete_expired_holder_value()
print("expired shared holder after nonlocal delete:", expired_holder_events)


nested_suspended_events = []


class NestedSuspendedTracked:
    def __del__(self):
        nested_suspended_events.append("finalized")


def delete_with_nested_suspended_alias():
    value = NestedSuspendedTracked()

    def leaf():
        alias = value
        yield None
        print("nested suspended leaf after resume:", nested_suspended_events)
        del alias

    def holder():
        nested = leaf()
        next(nested)
        yield None
        try:
            next(nested)
        except StopIteration:
            pass

    def delete_value():
        nonlocal value
        del value

    root = holder()
    next(root)
    delete_value()
    print("nested suspended after nonlocal delete:", nested_suspended_events)
    try:
        next(root)
    except StopIteration:
        pass
    print("nested suspended after root resume:", nested_suspended_events)


delete_with_nested_suspended_alias()


async_wrapper_owner_events = []


class AsyncWrapperOwnerTracked:
    def __del__(self):
        async_wrapper_owner_events.append("finalized")


def delete_with_async_wrapper_owner():
    value = AsyncWrapperOwnerTracked()

    async def holder():
        alias = value
        yield None
        print("async wrapper owner after resume:", async_wrapper_owner_events)
        del alias

    def delete_value():
        nonlocal value
        del value

    generator = holder()
    first = generator.__anext__()
    try:
        first.send(None)
    except StopIteration:
        pass
    del first
    awaitable = generator.__anext__()
    del generator
    delete_value()
    print("async wrapper owner after nonlocal delete:", async_wrapper_owner_events)
    try:
        awaitable.send(None)
    except StopAsyncIteration:
        pass
    print("async wrapper owner after wrapper resume:", async_wrapper_owner_events)


delete_with_async_wrapper_owner()
