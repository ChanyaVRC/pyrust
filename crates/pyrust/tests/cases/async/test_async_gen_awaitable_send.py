# Issue #3031: async-generator awaitables support their synchronous send API.


async def one_item():
    yield 42


class Pause:
    def __await__(self):
        received = yield "pause"
        return received


async def await_then_yield():
    received = await Pause()
    yield received


def drive(label, generator, awaitable, sent):
    try:
        awaitable.send(sent)
    except Exception as exc:
        value = exc.value if isinstance(exc, StopIteration) else None
        print(label, "first", type(exc).__name__, value)

    try:
        awaitable.send(None)
    except Exception as exc:
        print(label, "reuse", type(exc).__name__)

    try:
        generator.aclose().send(None)
    except StopIteration as exc:
        print(label, "close", exc.value, exc.args, repr(str(exc)))
    except Exception as exc:
        print(label, "close", type(exc).__name__)


g = one_item()
drive("anext", g, g.__anext__(), None)
g = one_item()
drive("asend-none", g, g.asend(None), None)
g = one_item()
drive("asend-value", g, g.asend(7), None)
g = one_item()
drive("anext-send-value", g, g.__anext__(), 7)

g = await_then_yield()
awaitable = g.__anext__()
print("inner await first:", awaitable.send(None))
try:
    awaitable.send(7)
except StopIteration as exc:
    print("inner await result:", exc.value)
try:
    awaitable.send(None)
except Exception as exc:
    print("inner await reuse:", type(exc).__name__)
try:
    g.aclose().send(None)
except StopIteration as exc:
    print("inner await close:", exc.value, exc.args, repr(str(exc)))

g = one_item()
close_awaitable = g.aclose()
try:
    close_awaitable.send(None)
except StopIteration as exc:
    print("close awaitable first:", exc.value, exc.args, repr(str(exc)))
try:
    close_awaitable.send(None)
except Exception as exc:
    print("close awaitable reuse:", type(exc).__name__, str(exc))

g = one_item()
throw_awaitable = g.athrow(ValueError)
try:
    throw_awaitable.send(None)
except Exception as exc:
    print("throw awaitable first:", type(exc).__name__)
try:
    throw_awaitable.send(None)
except Exception as exc:
    print("throw awaitable reuse:", type(exc).__name__, str(exc))


def show_call(label, call):
    try:
        result = call()
        print(label, "return", result)
    except Exception as exc:
        print(label, "raise", type(exc).__name__, str(exc))


g = one_item()
awaitable = g.__anext__()
show_call("fresh close", awaitable.close)
show_call("fresh close reuse", lambda: awaitable.send(None))

g = one_item()
awaitable = g.__anext__()
show_call("fresh throw", lambda: awaitable.throw(ValueError("boom")))
show_call("fresh throw reuse", lambda: awaitable.send(None))

g = await_then_yield()
awaitable = g.__anext__()
print("started close first:", awaitable.send(None))
show_call("started close", awaitable.close)
show_call("started close reuse", lambda: awaitable.send(None))

g = await_then_yield()
awaitable = g.__anext__()
print("started throw first:", awaitable.send(None))
show_call("started throw", lambda: awaitable.throw(ValueError("boom")))
show_call("started throw reuse", lambda: awaitable.send(None))


class CatchPause:
    def __await__(self):
        try:
            yield "pause"
        except ValueError:
            return "caught-inner"


async def catch_inner_throw():
    result = await CatchPause()
    yield result


g = catch_inner_throw()
awaitable = g.__anext__()
print("catching throw first:", awaitable.send(None))
show_call("catching throw", lambda: awaitable.throw(ValueError("boom")))
show_call("catching throw underlying", lambda: g.__anext__().send(None))

g = one_item()
awaitable = g.__anext__()
show_call("done first", lambda: awaitable.send(None))
show_call("done close", awaitable.close)
show_call("done throw", lambda: awaitable.throw(ValueError("boom")))

g = one_item()
awaitable = g.aclose()
show_call("driven aclose first", lambda: awaitable.send(None))
show_call("driven aclose throw", lambda: awaitable.throw(ValueError("boom")))

g = one_item()
awaitable = g.athrow(ValueError("seed"))
show_call("driven athrow first", lambda: awaitable.send(None))
show_call("driven athrow throw", lambda: awaitable.throw(ValueError("boom")))

g = one_item()
awaitable = g.aclose()
show_call("closed aclose first", awaitable.close)
show_call("closed aclose throw", lambda: awaitable.throw(ValueError("boom")))

g = one_item()
awaitable = g.athrow(ValueError("seed"))
show_call("closed athrow first", awaitable.close)
show_call("closed athrow throw", lambda: awaitable.throw(ValueError("boom")))


def show_advance(label, call):
    try:
        result = call()
        print(label, "return", result)
    except StopIteration as exc:
        print(
            label,
            "raise",
            type(exc).__name__,
            exc.value,
            exc.args,
            repr(str(exc)),
        )
    except Exception as exc:
        print(label, "raise", type(exc).__name__, str(exc))


def exercise_next(label, make_awaitable, use_dunder):
    generator = one_item()
    awaitable = make_awaitable(generator)
    if use_dunder:
        show_advance(label + " first", awaitable.__next__)
    else:
        show_advance(label + " first", lambda: next(awaitable))
    show_call(label + " send reuse", lambda: awaitable.send(None))
    show_call(
        label + " underlying",
        lambda: generator.__anext__().send(None),
    )
    show_advance(label + " cleanup", lambda: generator.aclose().send(None))


exercise_next("anext next", lambda generator: generator.__anext__(), False)
exercise_next("anext dunder", lambda generator: generator.__anext__(), True)
exercise_next("asend next", lambda generator: generator.asend(None), False)
exercise_next("asend dunder", lambda generator: generator.asend(None), True)

g = one_item()
awaitable = g.__anext__()
show_advance("anext send cross first", lambda: awaitable.send(None))
show_call("anext send cross next reuse", lambda: next(awaitable))
show_advance("anext send cross cleanup", lambda: g.aclose().send(None))

g = one_item()
awaitable = g.asend(None)
show_advance("asend send cross first", lambda: awaitable.send(None))
show_call("asend send cross next reuse", lambda: next(awaitable))
show_advance("asend send cross cleanup", lambda: g.aclose().send(None))

g = one_item()
awaitable = g.aclose()
show_advance("aclose next first", lambda: next(awaitable))
show_call("aclose next reuse", lambda: next(awaitable))

g = one_item()
awaitable = g.athrow(ValueError("boom"))
show_advance("athrow next first", lambda: next(awaitable))
show_call("athrow next reuse", lambda: next(awaitable))


def make_done_wrapper(kind, close_only):
    generator = one_item()
    if kind == "anext":
        awaitable = generator.__anext__()
    elif kind == "asend":
        awaitable = generator.asend(None)
    elif kind == "aclose":
        awaitable = generator.aclose()
    else:
        awaitable = generator.athrow(ValueError("seed"))
    if close_only:
        awaitable.close()
    else:
        try:
            awaitable.send(None)
        except Exception:
            pass
    return awaitable


for lifecycle in ("driven", "closed"):
    for kind in ("anext", "asend", "aclose", "athrow"):
        awaitable = make_done_wrapper(kind, lifecycle == "closed")
        show_call(
            lifecycle + " " + kind + " invalid throw",
            lambda awaitable=awaitable: awaitable.throw(42),
        )

for kind in ("anext", "aclose"):
    awaitable = make_done_wrapper(kind, True)
    show_call(kind + " done throw zero", lambda: awaitable.throw())
    show_call(
        kind + " done throw two",
        lambda: awaitable.throw(ValueError("x"), "y"),
    )
    show_call(
        kind + " done throw three",
        lambda: awaitable.throw(42, None, None),
    )
    show_call(
        kind + " done throw four",
        lambda: awaitable.throw(42, None, None, None),
    )
