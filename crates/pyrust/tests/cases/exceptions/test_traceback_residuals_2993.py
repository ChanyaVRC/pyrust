# Parity fixture for issue #2993:
#   1. PEP 709 comprehension inlining (no <listcomp>/<setcomp>/<dictcomp> frames)
#   2. PEP 654 except* residual re-raise & ExceptionGroup rewrap
#   3. Bundled asyncio frames do not report the script's filename
#   4. PEP 479 __cause__ / __context__ chain ordering and traceback ownership

import asyncio
import sys


def tb_frames(exc):
    out = []
    tb = exc.__traceback__
    while tb is not None:
        out.append((tb.tb_frame.f_code.co_name, tb.tb_lineno))
        tb = tb.tb_next
    return out


# ── 1. PEP 709 comprehension frames ──────────────────────────────────────────

def gen_boom():
    yield 1 / 0


def listcomp_caller():
    return [v for v in gen_boom()]


def setcomp_caller():
    return {v for v in gen_boom()}


def dictcomp_caller():
    return {v: v for v in gen_boom()}


def comp_elt_caller():
    return [1 // x for x in [0]]


def comp_cond_caller():
    return [x for x in [0] if 1 // x]


def nested_comp_caller():
    return [[v for v in gen_boom()] for _ in [1]]


for label, fn in [
    ("listcomp", listcomp_caller),
    ("setcomp", setcomp_caller),
    ("dictcomp", dictcomp_caller),
    ("elt", comp_elt_caller),
    ("cond", comp_cond_caller),
    ("nested", nested_comp_caller),
]:
    try:
        fn()
    except ZeroDivisionError as e:
        names = [name for name, _ in tb_frames(e)]
        print("comp", label, names)
        assert "<listcomp>" not in names
        assert "<setcomp>" not in names
        assert "<dictcomp>" not in names


def getframe_in_comp():
    return [sys._getframe().f_code.co_name for _ in [1]]


print("getframe_in_comp", getframe_in_comp())


async def async_gen_boom():
    yield 1 / 0


async def async_comp_caller():
    return [v async for v in async_gen_boom()]


try:
    asyncio.run(async_comp_caller())
except ZeroDivisionError as e:
    names = [name for name, _ in tb_frames(e)]
    assert "<listcomp>" not in names
    assert "async_comp_caller" in names and "async_gen_boom" in names
    print("async_comp_ok", "async_comp_caller" in names, "async_gen_boom" in names)


# ── 2. except* residual re-raise & ExceptionGroup rewrap ─────────────────────

# 2a: unmatched plain exception stays unwrapped with suppress=False.
try:
    try:
        raise ValueError("plain")
    except* TypeError:
        pass
except BaseException as e:
    print("2a", type(e).__name__, str(e), e.__context__, e.__suppress_context__)

# 2b: matched plain exception + bare raise re-wraps into ExceptionGroup('', [exc]).
try:
    try:
        for _ in gen_boom():
            pass
    except* ZeroDivisionError:
        raise
except BaseException as e:
    print(
        "2b",
        type(e).__name__,
        len(e.exceptions),
        type(e.exceptions[0]).__name__,
        tb_frames(e),
        [name for name, _ in tb_frames(e.exceptions[0])],
    )

# 2c: matched plain exception + bare raise across a function boundary omits
# the re-raising function's own frame from the outer group's traceback.
def except_star_fn():
    try:
        for _ in gen_boom():
            pass
    except* ZeroDivisionError:
        raise


try:
    except_star_fn()
except BaseException as e:
    print(
        "2c",
        type(e).__name__,
        [name for name, _ in tb_frames(e)],
        [name for name, _ in tb_frames(e.exceptions[0])],
    )

# 2d: matched plain exception + new raise sets __context__ to the wrapped group.
try:
    try:
        raise ValueError("v")
    except* ValueError:
        raise KeyError("k")
except BaseException as e:
    print(
        "2d",
        type(e).__name__,
        type(e.__context__).__name__,
        [type(x).__name__ for x in e.__context__.exceptions],
    )

# 2e: partial match on ExceptionGroup + bare raise merges back with orig.message.
try:
    try:
        raise ExceptionGroup("eg", [ValueError("v"), TypeError("t")])
    except* ValueError:
        raise
except BaseException as e:
    print(
        "2e",
        type(e).__name__,
        e.message,
        [type(x).__name__ for x in e.exceptions],
        e.__suppress_context__,
    )

# 2f: partial match + new raise combines new exception and residual group.
try:
    try:
        raise ExceptionGroup("eg", [ValueError("v"), TypeError("t")])
    except* ValueError:
        raise KeyError("k")
except BaseException as e:
    print(
        "2f",
        type(e).__name__,
        len(e.exceptions),
        type(e.exceptions[0]).__name__,
        type(e.exceptions[0].__context__).__name__,
        [type(x).__name__ for x in e.exceptions[0].__context__.exceptions],
        type(e.exceptions[1]).__name__,
        [type(x).__name__ for x in e.exceptions[1].exceptions],
    )

# 2g: two handlers both raising combine into ExceptionGroup('', [k1, k2]).
try:
    try:
        raise ExceptionGroup("eg", [ValueError("v"), TypeError("t")])
    except* ValueError:
        raise KeyError("k1")
    except* TypeError:
        raise IndexError("k2")
except BaseException as e:
    print(
        "2g",
        type(e).__name__,
        [(type(x).__name__, [type(c).__name__ for c in x.__context__.exceptions]) for x in e.exceptions],
    )

# 2h: single-element ExceptionGroup fully matched + new raise surfaces unwrapped.
try:
    try:
        raise ExceptionGroup("eg", [ValueError("v")])
    except* ValueError:
        raise KeyError("k")
except BaseException as e:
    print(
        "2h",
        type(e).__name__,
        type(e.__context__).__name__,
        [type(x).__name__ for x in e.__context__.exceptions],
    )


# ── 3. Bundled asyncio frames do not report the script's filename ────────────

async def async_boom():
    raise ValueError("async-boom")


try:
    asyncio.run(async_boom())
except ValueError as e:
    script_file = __file__
    tb = e.__traceback__
    user_frames = []
    lib_frames_report_script = []
    while tb is not None:
        co = tb.tb_frame.f_code
        if co.co_name in ("<module>", "async_boom"):
            user_frames.append((co.co_name, co.co_filename == script_file))
        else:
            if co.co_filename == script_file:
                lib_frames_report_script.append((co.co_name, co.co_filename))
        tb = tb.tb_next
    print("3_user", user_frames)
    print("3_lib_mismatch", lib_frames_report_script)


# ── 4. PEP 479 chain ordering & traceback ownership ──────────────────────────

def gen_stop():
    raise StopIteration("s")
    yield 1


try:
    next(gen_stop())
except RuntimeError as e:
    print(
        "4a",
        str(e),
        e.__cause__ is e.__context__,
        e.__suppress_context__,
        type(e.__cause__).__name__,
        [name for name, _ in tb_frames(e)],
        [name for name, _ in tb_frames(e.__cause__)],
    )


def stop_helper():
    raise StopIteration("deep")


def gen_deep_stop():
    stop_helper()
    yield 1


try:
    for _ in gen_deep_stop():
        pass
except RuntimeError as e:
    print(
        "4b",
        str(e),
        [name for name, _ in tb_frames(e)],
        [name for name, _ in tb_frames(e.__cause__)],
    )


def gen_stop_in_except():
    try:
        raise KeyError("orig")
    except KeyError:
        raise StopIteration("s")
    yield 1


try:
    next(gen_stop_in_except())
except RuntimeError as e:
    cause = e.__cause__
    orig = cause.__context__
    print(
        "4c",
        type(orig).__name__,
        [name for name, _ in tb_frames(orig)],
        type(cause).__name__,
        [name for name, _ in tb_frames(cause)],
        type(e).__name__,
        [name for name, _ in tb_frames(e)],
    )


async def agen_stop():
    raise StopAsyncIteration("sa")
    yield 1


async def drive_agen_stop():
    async for _ in agen_stop():
        pass


try:
    asyncio.run(drive_agen_stop())
except RuntimeError as e:
    print(
        "4d",
        str(e),
        e.__cause__ is e.__context__,
        e.__suppress_context__,
        type(e.__cause__).__name__,
        [name for name, _ in tb_frames(e.__cause__)],
        "agen_stop" in [name for name, _ in tb_frames(e)],
    )
