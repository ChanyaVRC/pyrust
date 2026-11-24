# Structural traceback parity (`tb_lineno`, `co_name`, `co_firstlineno`) for
# issue #2992: await / yield from / yield, multi-line calls, and `<genexpr>`
# element / filter / nested raises.


def frames_of(fn):
    try:
        fn()
    except BaseException as exc:
        out = []
        tb = exc.__traceback__
        while tb is not None:
            out.append((tb.tb_frame.f_code.co_name, tb.tb_lineno))
            tb = tb.tb_next
        return type(exc).__name__, str(exc), out[1:]
    raise AssertionError("expected exception")


# --- 1. yield from / yield / await ------------------------------------------


def sub_raise():
    raise RuntimeError("boom")
    yield 1


def yf_single():
    x = yield from sub_raise()
    return x


def yf_multiline():
    x = (
        yield from sub_raise()
    )
    return x


def yf_bad_iter():
    x = yield from 123
    return x


def sub_yield():
    yield 1


def yf_throw_outer():
    x = yield from sub_yield()
    return x


def yield_throw_fn():
    x = yield 1
    return x


async def bad_coro():
    raise RuntimeError("async boom")


async def await_single():
    return await bad_coro()


async def await_multiline():
    x = await bad_coro(
    )
    return x


async def await_bad_target():
    x = await 123
    return x


print("yf_single:", frames_of(lambda: next(yf_single())))
print("yf_multiline:", frames_of(lambda: next(yf_multiline())))
print("yf_bad_iter:", frames_of(lambda: next(yf_bad_iter())))


def run_yf_throw():
    g = yf_throw_outer()
    next(g)
    g.throw(ValueError("thrown"))


print("yf_throw:", frames_of(run_yf_throw))


def run_yield_throw():
    g = yield_throw_fn()
    next(g)
    g.throw(ValueError("thrown2"))


print("yield_throw:", frames_of(run_yield_throw))
print("await_single:", frames_of(lambda: await_single().send(None)))
print("await_multiline:", frames_of(lambda: await_multiline().send(None)))
print("await_bad_target:", frames_of(lambda: await_bad_target().send(None)))


# --- 2. Multi-line calls ----------------------------------------------------


def inner_raise(a, b=0):
    raise ValueError("fail")


def ml_call():
    return inner_raise(
        1,
        2,
    )


def ml_kw_call():
    return inner_raise(
        1,
        b=2,
    )


class Box:
    def boom(self, a):
        raise ValueError("box")


def ml_method():
    b = Box()
    return b.boom(
        1,
    )


def ml_chained_attr():
    b = Box()
    return (
        b
        .boom(
            1,
        )
    )


def passthrough(x):
    return x


def ml_nested_call():
    return passthrough(
        inner_raise(
            1,
        )
    )


print("ml_call:", frames_of(ml_call))
print("ml_kw_call:", frames_of(ml_kw_call))
print("ml_method:", frames_of(ml_method))
print("ml_chained_attr:", frames_of(ml_chained_attr))
print("ml_nested_call:", frames_of(ml_nested_call))


# --- 3. <genexpr> tb_lineno and co_firstlineno ------------------------------


def ge_single():
    g = (1 // x for x in [0])
    return list(g)


def ge_multiline():
    g = (
        1 // x
        for x in [0]
    )
    return list(g)


def ge_filter():
    g = (
        x
        for x in [0]
        if 1 // x
    )
    return list(g)


def ge_multi_if():
    g = (
        x
        for x in [0]
        if True
        if 1 // x
    )
    return list(g)


def ge_nested():
    g = (
        (
            1 // y
            for y in [x]
        )
        for x in [0]
    )
    for inner in g:
        list(inner)


def ge_sum():
    return sum(
        1 // x
        for x in [0]
    )


def ge_any():
    return any(
        1 // x
        for x in [0]
    )


def ge_next():
    return next(
        1 // x
        for x in [0]
    )


def ge_firstlineno_check():
    g = (
        x
        for x in [1]
    )
    return g.gi_frame.f_code.co_firstlineno


print("ge_single:", frames_of(ge_single))
print("ge_multiline:", frames_of(ge_multiline))
print("ge_filter:", frames_of(ge_filter))
print("ge_multi_if:", frames_of(ge_multi_if))
print("ge_nested:", frames_of(ge_nested))
print("ge_sum:", frames_of(ge_sum))
print("ge_any:", frames_of(ge_any))
print("ge_next:", frames_of(ge_next))
print("ge_firstlineno:", ge_firstlineno_check())
