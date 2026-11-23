# Issue #3029:
#   1. `tb_frame.f_locals` for unwound & catching function / generator / class
#      / module frames (including cellvars, freevars, live catching frame
#      identity, and escaped catching exceptions).
#   2. `yield from` / `await` delegation frames present with their real
#      `f_code.co_name` in `sys._getframe(depth)` and `f.f_back` walks across
#      `next()`, `for` loops, `.throw()`, `asyncio.run()`, and manual coroutine
#      driving.
#   3. PEP 709 inlined comprehension frames (`<listcomp>`, `<setcomp>`,
#      `<dictcomp>`, sync and async) skipped in `sys._getframe()` and `f_back`
#      walks, while `<genexpr>` (sync and async) remains a real frame.

import asyncio
import sys


# ── 1. tb_frame.f_locals & identity ──────────────────────────────────────────

def make_raiser(cell_arg):
    captured = cell_arg * 10

    def inner(p):
        local_val = p + captured
        1 / 0

    def middle(m):
        mid_val = m + 1
        inner(mid_val)

    return middle


def catch_in_fn(arg):
    before_try = arg + 1
    try:
        make_raiser(3)(arg)
    except ZeroDivisionError as e:
        tb = e.__traceback__
        live_same = tb.tb_frame is sys._getframe()
        after_catch = "set-in-except"
        live_names = []
        cur = tb
        while cur is not None:
            live_names.append(cur.tb_frame.f_code.co_name)
            cur = cur.tb_next
        middle_locals = dict(tb.tb_next.tb_frame.f_locals)
        inner_locals = dict(tb.tb_next.tb_next.tb_frame.f_locals)
        return e, live_same, live_names, middle_locals, inner_locals


caught_exc, live_same, live_names, middle_locals, inner_locals = catch_in_fn(5)
print("catch_in_fn live_same:", live_same)
print("catch_in_fn chain:", live_names)
print(
    "middle_locals:",
    sorted((k, v.__name__ if callable(v) else v) for k, v in middle_locals.items()),
)
print("inner_locals:", sorted(inner_locals.items()))
escaped_catch_locals = caught_exc.__traceback__.tb_frame.f_locals
print(
    "escaped catch locals:",
    escaped_catch_locals["arg"],
    escaped_catch_locals["before_try"],
    escaped_catch_locals["after_catch"],
    "e" in escaped_catch_locals,
)


# Exception escapes catching function BEFORE `e.__traceback__` is ever read.
def catch_and_return_deferred():
    c = 42
    try:
        1 / 0
    except ZeroDivisionError as exc:
        after_bind = 99
        return exc


deferred_exc = catch_and_return_deferred()
deferred_locals = deferred_exc.__traceback__.tb_frame.f_locals
print(
    "deferred escaped catch locals:",
    deferred_locals["c"],
    deferred_locals["after_bind"],
    "exc" in deferred_locals,
)


# Pre-grabbed `_getframe()` in an unwinding function preserves `tb_frame` identity.
saved_unwound_frame = None


def unwound_with_frame(x):
    global saved_unwound_frame
    saved_unwound_frame = sys._getframe()
    y = x + 2
    1 / 0


try:
    unwound_with_frame(7)
except ZeroDivisionError as e:
    unwound_tb = e.__traceback__.tb_next
    print(
        "unwound identity:",
        unwound_tb.tb_frame is saved_unwound_frame,
        sorted(unwound_tb.tb_frame.f_locals.items()),
    )


# Generator raising frame locals.
def raising_gen(start):
    step = start + 1
    yield step
    after_yield = step * 2
    1 / 0


g = raising_gen(10)
next(g)
try:
    next(g)
except ZeroDivisionError as e:
    gen_tb = e.__traceback__.tb_next
    print(
        "gen raise tb:",
        gen_tb.tb_frame.f_code.co_name,
        sorted(gen_tb.tb_frame.f_locals.items()),
    )


# Coroutine raising frame locals.
async def raising_coro(val):
    local_c = val + 5
    1 / 0


try:
    asyncio.run(raising_coro(4))
except ZeroDivisionError as e:
    tb = e.__traceback__
    while tb.tb_next is not None:
        tb = tb.tb_next
    print(
        "coro raise tb:",
        tb.tb_frame.f_code.co_name,
        sorted(tb.tb_frame.f_locals.items()),
    )


# Class body raising & catching frame locals.
try:
    class BoomClass:
        alpha = 1
        beta = 2
        1 / 0
except ZeroDivisionError as e:
    tb = e.__traceback__
    print("module tb f_locals is f_globals:", tb.tb_frame.f_locals is tb.tb_frame.f_globals)
    cls_tb = tb.tb_next
    print(
        "class raise tb:",
        cls_tb.tb_frame.f_code.co_name,
        sorted(
            (k, v)
            for k, v in cls_tb.tb_frame.f_locals.items()
            if not k.startswith("__")
        ),
    )


class CatchClass:
    x = 10
    try:
        y = 20
        1 / 0
    except ZeroDivisionError as e:
        catch_cls_tb = e.__traceback__
        catch_cls_same = catch_cls_tb.tb_frame is sys._getframe()
        catch_cls_name = catch_cls_tb.tb_frame.f_code.co_name
        catch_cls_keys = sorted(
            k for k in catch_cls_tb.tb_frame.f_locals if k in ("x", "y")
        )


print(
    "class catch tb:",
    CatchClass.catch_cls_same,
    CatchClass.catch_cls_name,
    CatchClass.catch_cls_keys,
)


# ── 2. yield from / await delegation _getframe walks ─────────────────────────

def collect_stack_until(stop_name):
    by_depth = []
    i = 0
    while True:
        f = sys._getframe(i)
        name = f.f_code.co_name
        by_depth.append(name)
        if name == stop_name:
            break
        i += 1
    by_back = []
    cur = sys._getframe(0)
    while cur is not None:
        name = cur.f_code.co_name
        by_back.append(name)
        if name == stop_name:
            break
        cur = cur.f_back
    return by_depth, by_back, by_depth == by_back


def leaf_gen():
    leaf_local = "leaf"
    yield collect_stack_until("drive_yield_from")


def mid_gen():
    mid_local = "mid"
    yield from leaf_gen()


def outer_gen():
    outer_local = "outer"
    yield from mid_gen()


def drive_yield_from():
    via_next = next(outer_gen())
    via_for = None
    for item in outer_gen():
        via_for = item
    return via_next, via_for


yf_next, yf_for = drive_yield_from()
print("yield from next:", yf_next)
print("yield from for:", yf_for)


def leaf_throw_gen():
    try:
        yield "ready"
    except ValueError:
        yield collect_stack_until("drive_yield_from_throw")


def mid_throw_gen():
    yield from leaf_throw_gen()


def outer_throw_gen():
    yield from mid_throw_gen()


def drive_yield_from_throw():
    gen = outer_throw_gen()
    assert next(gen) == "ready"
    return gen.throw(ValueError("boom"))


print("yield from throw:", drive_yield_from_throw())


async def leaf_coro():
    return collect_stack_until("outer_coro")


async def mid_coro():
    return await leaf_coro()


async def outer_coro():
    return await mid_coro()


print("asyncio.run await chain:", asyncio.run(outer_coro()))


def drive_manual_coro():
    c = outer_coro()
    try:
        c.send(None)
    except StopIteration as stop:
        return stop.value


print("manual coro send chain:", drive_manual_coro())


async def leaf_throw_coro():
    try:
        await _YieldOnceCoro()
    except ValueError:
        return collect_stack_until("drive_manual_coro_throw")


class _YieldOnceCoro:
    def __await__(self):
        yield "paused"


async def mid_throw_coro():
    return await leaf_throw_coro()


async def outer_throw_coro():
    return await mid_throw_coro()


def drive_manual_coro_throw():
    c = outer_throw_coro()
    assert c.send(None) == "paused"
    try:
        c.throw(ValueError("coro-boom"))
    except StopIteration as stop:
        return stop.value


print("manual coro throw chain:", drive_manual_coro_throw())


# ── 3. PEP 709 comprehension frames skipped in _getframe ─────────────────────

# Module-scope comprehensions see `<module>` at depth 0, and depth 1 raises.
mod_listcomp = [sys._getframe(0).f_code.co_name for _ in range(1)]
mod_setcomp = sorted({sys._getframe(0).f_code.co_name for _ in range(1)})
mod_dictcomp = {0: sys._getframe(0).f_code.co_name for _ in range(1)}
mod_depth1_err = None
try:
    [sys._getframe(1) for _ in range(1)]
except ValueError as e:
    mod_depth1_err = str(e)
print("module comps:", mod_listcomp, mod_setcomp, mod_dictcomp, mod_depth1_err)


def comp_outer():
    def helper_from_comp():
        return (
            sys._getframe(0).f_code.co_name,
            sys._getframe(1).f_code.co_name,
            sys._getframe(0).f_back.f_code.co_name,
            sys._getframe(2).f_code.co_name,
        )

    def comp_fn():
        f = sys._getframe(0)
        lc = [
            (
                sys._getframe(0).f_code.co_name,
                sys._getframe(0) is f,
                sys._getframe(1).f_code.co_name,
            )
            for _ in range(1)
        ]
        sc = {
            (
                sys._getframe(0).f_code.co_name,
                sys._getframe(0) is f,
                sys._getframe(1).f_code.co_name,
            )
            for _ in range(1)
        }
        dc = {
            k: (
                sys._getframe(0).f_code.co_name,
                sys._getframe(0) is f,
                sys._getframe(1).f_code.co_name,
            )
            for k in range(1)
        }
        nested = [
            [sys._getframe(0).f_code.co_name for _ in range(1)]
            for _ in range(1)
        ]
        via_helper = [helper_from_comp() for _ in range(1)]
        ge = list(
            (
                sys._getframe(0).f_code.co_name,
                sys._getframe(1).f_code.co_name,
                sys._getframe(0).f_back.f_code.co_name,
            )
            for _ in range(1)
        )
        comp_in_ge = list(
            [sys._getframe(0).f_code.co_name for _ in range(1)]
            for _ in range(1)
        )
        ge_in_comp = [
            x
            for x in (
                (
                    sys._getframe(0).f_code.co_name,
                    sys._getframe(1).f_code.co_name,
                    sys._getframe(0).f_back.f_code.co_name,
                )
                for _ in range(1)
            )
        ]
        return lc, sorted(sc), dc, nested, via_helper, ge, comp_in_ge, ge_in_comp

    return comp_fn()


print("fn comps:", comp_outer())


class CompInClass:
    lc = [sys._getframe(0).f_code.co_name for _ in range(1)]
    sc = sorted({sys._getframe(0).f_code.co_name for _ in range(1)})
    dc = {0: sys._getframe(0).f_code.co_name for _ in range(1)}


print("class comps:", CompInClass.lc, CompInClass.sc, CompInClass.dc)


async def async_comp_outer():
    async def aiter():
        yield 1

    async def async_comp_fn():
        f = sys._getframe(0)
        alc = [
            (
                sys._getframe(0).f_code.co_name,
                sys._getframe(0) is f,
                sys._getframe(1).f_code.co_name,
            )
            async for _ in aiter()
        ]
        asc = {
            (
                sys._getframe(0).f_code.co_name,
                sys._getframe(0) is f,
                sys._getframe(1).f_code.co_name,
            )
            async for _ in aiter()
        }
        adc = {
            0: (
                sys._getframe(0).f_code.co_name,
                sys._getframe(0) is f,
                sys._getframe(1).f_code.co_name,
            )
            async for _ in aiter()
        }
        age = [
            x
            async for x in (
                (
                    sys._getframe(0).f_code.co_name,
                    sys._getframe(1).f_code.co_name,
                )
                async for _ in aiter()
            )
        ]
        return alc, sorted(asc), adc, age

    return await async_comp_fn()


print("async comps:", asyncio.run(async_comp_outer()))
