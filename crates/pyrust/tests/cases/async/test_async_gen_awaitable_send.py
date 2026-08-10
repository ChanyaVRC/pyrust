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
        print(label, "close", exc.value)
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
    print("inner await close:", exc.value)

g = one_item()
close_awaitable = g.aclose()
try:
    close_awaitable.send(None)
except StopIteration as exc:
    print("close awaitable first:", exc.value)
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
