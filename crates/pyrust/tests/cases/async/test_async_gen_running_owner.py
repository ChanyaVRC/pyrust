# Issue #3031: an async-generator awaitable keeps exclusive ownership while
# the generator is suspended inside an inner await.


class Pause:
    def __await__(self):
        value = yield "pause"
        return value


async def values():
    value = await Pause()
    yield value
    yield "tail"


def show(label, call):
    try:
        print(label, "return", call())
    except BaseException as exc:
        print(
            label,
            "raise",
            type(exc).__name__,
            getattr(exc, "value", None),
            str(exc),
        )


def make_running():
    generator = values()
    first = generator.__anext__()
    show("first pause", lambda: first.send(None))
    return generator, first


def make_second(generator, kind):
    if kind == "anext":
        return generator.__anext__()
    if kind == "asend":
        return generator.asend(None)
    if kind == "aclose":
        return generator.aclose()
    return generator.athrow(ValueError("seed"))


# A distinct wrapper cannot drive the generator while `first` owns its
# inner-await suspension. Failed anext/asend wrappers remain retryable after
# the owner completes; failed aclose/athrow wrappers are one-shot failures.
for kind in ("anext", "asend", "aclose", "athrow"):
    generator, first = make_running()
    second = make_second(generator, kind)
    show(kind + " overlap", lambda second=second: second.send(None))
    show(kind + " owner finish", lambda first=first: first.send(7))
    show(kind + " retry", lambda second=second: second.send(None))


# The next()/__next__ route shares the same persistent ownership check.
generator, first = make_running()
second = generator.__anext__()
show("next overlap", lambda: next(second))
show("next owner finish", lambda: first.send(7))
show("next retry", lambda: next(second))


# Completing or erroring the owning wrapper releases ownership.
generator, first = make_running()
show("completed owner finish", lambda: first.send(7))
fresh = generator.__anext__()
show("completed owner fresh", lambda: fresh.send(None))

generator, first = make_running()
show("errored owner finish", lambda: first.throw(LookupError("boom")))
fresh = generator.__anext__()
show("errored owner fresh", lambda: fresh.send(None))


# Closing or dropping an owner parked at an inner await does not release the
# generator: CPython retains the occupied owner slot even after its Weak dies.
generator, first = make_running()
first.close()
show("closed owner reuse", lambda: first.send(None))
fresh = generator.__anext__()
show("closed owner fresh", lambda: fresh.send(None))

generator, first = make_running()
del first
fresh = generator.__anext__()
show("dropped owner fresh", lambda: fresh.send(None))


# A just-started asend validation error is terminal and releases its claim.
generator = values()
invalid = generator.asend(5)
show("invalid owner", lambda: invalid.send(None))
fresh = generator.__anext__()
show("invalid owner fresh pause", lambda: fresh.send(None))
show("invalid owner fresh finish", lambda: fresh.send(3))


# Direct awaitable.throw() is the CPython exception: it bypasses the persistent
# owner claim for that step. It neither replaces a distinct existing owner nor
# claims an empty owner slot when it suspends at an inner await.
async def direct_throw_with_owner():
    try:
        value = await Pause()
    except ValueError:
        value = await Pause()
    yield value


generator = direct_throw_with_owner()
first = generator.__anext__()
show("direct existing owner pause", lambda: first.send(None))
second = generator.__anext__()
show("direct distinct throw", lambda: second.throw(ValueError("injected")))
show("direct distinct send", lambda: second.send(None))
show("direct existing owner finish", lambda: first.send(7))


async def direct_throw_without_owner():
    try:
        yield "seed"
    except ValueError:
        value = await Pause()
        yield value


generator = direct_throw_without_owner()
seed = generator.__anext__()
show("direct empty seed", lambda: seed.send(None))
thrown = generator.__anext__()
show("direct empty throw", lambda: thrown.throw(ValueError("injected")))
other = generator.__anext__()
show("direct empty other", lambda: other.send(9))


# The transient RefCell-borrow and GenDriving paths use the same family label
# as the persistent owner conflict.
for kind in ("anext", "asend", "aclose", "athrow"):
    holder = [None]

    async def reentrant():
        generator = holder[0]
        nested = make_second(generator, kind)
        nested.send(None)
        yield 1

    generator = reentrant()
    holder[0] = generator
    outer = generator.__anext__()
    show(kind + " active", lambda outer=outer: outer.send(None))
