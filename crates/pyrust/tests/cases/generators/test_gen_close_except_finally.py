# Issue #3357: an enclosing `finally:` must run when an exception leaves an
# `except` handler body -- in particular when `generator.close()` /
# `generator.throw()` inject an exception at a `yield` suspended inside the
# handler body (also through `with`, nested `try`, `yield from`, and async
# generators).  The compiler used to pop the finally handler before every
# `except` body and only inlined the finally block at explicit `raise` sites,
# so injected and implicitly raised exceptions skipped it (and an explicit
# `raise` inside an `except*` body ran it twice).
import asyncio
import sys


def section(name):
    print(f"--- {name}")


def exc_name():
    exc = sys.exc_info()[1]
    return None if exc is None else type(exc).__name__


# 1. The issue reproducer: close() while suspended inside `except`.
section("close in except")


def gen_issue():
    try:
        yield 1
        raise ValueError
    except ValueError:
        yield 2
    finally:
        print("fin", exc_name())


it = gen_issue()
print(next(it))
print(next(it))
print("close ->", it.close())
print("after close:", list(it))


# 2. throw() while suspended inside `except`: finally runs, exception escapes.
section("throw in except")


def gen_throw():
    try:
        yield 1
        raise ValueError("v")
    except ValueError as e:
        print("handling", repr(e))
        yield 2
        print("not reached")
    finally:
        print("fin", exc_name())


it = gen_throw()
next(it)
next(it)
try:
    it.throw(KeyError("k"))
except KeyError as e:
    print("caught", repr(e), "context:", repr(e.__context__))
print("after throw:", list(it))

# Throwing the same type the suspended handler matched is not re-caught by it.
it = gen_throw()
next(it)
next(it)
try:
    it.throw(ValueError("again"))
except ValueError as e:
    print("caught", repr(e), "context:", repr(e.__context__))


# 3. try / except / else / finally: suspended in each region in turn.
section("except else finally")


def gen_full(fail):
    try:
        print("body")
        yield "body"
        if fail:
            raise ValueError
    except ValueError:
        yield "except"
    else:
        yield "else"
    finally:
        print("fin")
    print("after try")


for fail in (False, True):
    for steps in (1, 2):
        it = gen_full(fail)
        got = [next(it) for _ in range(steps)]
        print(fail, steps, got)
        it.close()


# 4. Several handlers: suspended in the second one; handler exceptions run
#    finally exactly once.
section("multiple handlers")


def gen_multi(exc):
    try:
        yield 0
        raise exc
    except KeyError:
        yield "key"
    except (TypeError, ValueError) as e:
        yield type(e).__name__
    finally:
        print("fin", exc_name())


for exc in (KeyError, TypeError, ValueError):
    it = gen_multi(exc)
    next(it)
    print(next(it))
    it.close()


# 5. Nested try inside the handler body: inner finally first, then outer.
section("nested try in except")


def gen_nested():
    try:
        yield 1
        raise ValueError
    except ValueError:
        try:
            yield 2
        finally:
            print("inner fin", exc_name())
    finally:
        print("outer fin", exc_name())


it = gen_nested()
next(it)
next(it)
it.close()

it = gen_nested()
next(it)
next(it)
try:
    it.throw(RuntimeError("boom"))
except RuntimeError as e:
    print("caught", repr(e))


# 6. Nested try/except inside the handler body that does not match.
section("nested except no match")


def gen_nested_except():
    try:
        yield 1
        raise ValueError
    except ValueError:
        try:
            yield 2
        except KeyError:
            print("wrong handler")
    finally:
        print("fin")


it = gen_nested_except()
next(it)
next(it)
it.close()


# 7. `with` inside the handler body: __exit__ sees GeneratorExit, then finally.
section("with in except")


class CM:
    def __init__(self, name):
        self.name = name

    def __enter__(self):
        print("enter", self.name)
        return self

    def __exit__(self, typ, val, tb):
        print("exit", self.name, None if typ is None else typ.__name__)
        return False


def gen_with():
    try:
        yield 1
        raise ValueError
    except ValueError:
        with CM("a"):
            yield 2
    finally:
        print("fin")


it = gen_with()
next(it)
next(it)
it.close()


# 8. Suspended inside except inside a `with` body (the with wraps the try).
section("except in with")


def gen_with_outer():
    with CM("outer"):
        try:
            yield 1
            raise ValueError
        except ValueError:
            yield 2
        finally:
            print("fin")


it = gen_with_outer()
next(it)
next(it)
it.close()


# 9. Catching GeneratorExit and yielding -> RuntimeError; a later close()
#    then finishes the generator and runs finally.
section("ignored GeneratorExit")


def gen_ignore():
    try:
        yield 1
        raise ValueError
    except ValueError:
        try:
            yield 2
        except GeneratorExit:
            print("caught GeneratorExit")
            yield 3
    finally:
        print("fin", exc_name())


it = gen_ignore()
next(it)
next(it)
try:
    it.close()
except RuntimeError as e:
    print("RuntimeError:", e)
print("still suspended:", it.gi_frame is not None, it.gi_suspended)
print("close again ->", it.close())
print("after:", list(it), it.gi_frame)


class Suspend:
    def __await__(self):
        yield "suspended"


async def coro_ignore():
    try:
        raise ValueError
    except ValueError:
        try:
            await Suspend()
        except GeneratorExit:
            await Suspend()
    finally:
        print("coro fin", exc_name())


c = coro_ignore()
c.send(None)
try:
    c.close()
except RuntimeError as e:
    print("RuntimeError:", e)
print("coro still suspended:", c.cr_suspended)
c.close()


# 10. The finally block itself yields during close() -> RuntimeError.
section("finally yields on close")


def gen_finally_yields():
    try:
        yield 1
        raise ValueError
    except ValueError:
        yield 2
    finally:
        print("fin")
        yield 3


it = gen_finally_yields()
next(it)
next(it)
try:
    it.close()
except RuntimeError as e:
    print("RuntimeError:", e)


# 11. An outer `except GeneratorExit` sees the injected exception after the
#     inner finally ran.
section("except GeneratorExit outside")


def gen_ge_outer():
    try:
        try:
            yield 1
            raise ValueError
        except ValueError:
            yield 2
        finally:
            print("inner fin")
    except GeneratorExit:
        print("outer caught GeneratorExit")
        raise
    finally:
        print("outer fin")


it = gen_ge_outer()
next(it)
next(it)
it.close()


# 12. `return` in finally swallows the injected GeneratorExit.
section("finally return on close")


def gen_finally_return():
    try:
        yield 1
        raise ValueError
    except ValueError:
        yield 2
    finally:
        return "swallowed"


it = gen_finally_return()
next(it)
next(it)
print("close ->", it.close())


# 13. `yield from` inside the handler body: close()/throw() reach the
#     sub-generator first, then the outer finally.
section("yield from in except")


def sub():
    try:
        yield "sub1"
        yield "sub2"
    finally:
        print("sub fin", exc_name())


def gen_yield_from():
    try:
        yield 1
        raise ValueError
    except ValueError:
        result = yield from sub()
        print("sub returned", result)
    finally:
        print("outer fin", exc_name())


it = gen_yield_from()
next(it)
print(next(it))
it.close()

it = gen_yield_from()
next(it)
print(next(it))
try:
    it.throw(KeyError("thrown"))
except KeyError as e:
    print("caught", repr(e))


def sub_handles():
    try:
        yield "sub"
    except KeyError:
        print("sub handled KeyError")
    return "sub-result"


def gen_yield_from_handled():
    try:
        yield 1
        raise ValueError
    except ValueError:
        result = yield from sub_handles()
        print("sub returned", result)
        yield "after sub"
    finally:
        print("outer fin", exc_name())


it = gen_yield_from_handled()
next(it)
print(next(it))
print(it.throw(KeyError))
it.close()


# 14. return/break/continue out of a handler body still run finally once.
section("early exits")


def gen_return():
    try:
        yield 1
        raise ValueError
    except ValueError:
        yield 2
        return "ret"
    finally:
        print("fin")


it = gen_return()
next(it)
next(it)
try:
    next(it)
except StopIteration as e:
    print("StopIteration", e.value)


def loop_exits():
    out = []
    for i in range(4):
        try:
            raise ValueError(i)
        except ValueError:
            if i == 1:
                continue
            if i == 3:
                break
            out.append(i)
        finally:
            out.append(f"fin{i}")
    return out


print(loop_exits())


def return_from_except():
    try:
        raise ValueError
    except ValueError:
        return "except-return"
    finally:
        print("fin")


print(return_from_except())


# 15. Non-generator code: exceptions leaving the handler body run finally.
section("plain functions")


def implicit():
    try:
        raise ValueError("first")
    except ValueError:
        int("x")
    finally:
        print("fin", exc_name())


try:
    implicit()
except ValueError as e:
    print("caught", repr(e), "context:", repr(e.__context__))


def explicit_raise():
    try:
        raise ValueError("first")
    except ValueError:
        raise KeyError("second")
    finally:
        print("fin", exc_name())


try:
    explicit_raise()
except KeyError as e:
    print("caught", repr(e), "context:", repr(e.__context__))


def bare_raise():
    try:
        raise ValueError("first")
    except ValueError:
        raise
    finally:
        print("fin", exc_name())


try:
    bare_raise()
except ValueError as e:
    print("caught", repr(e))


def raise_in_finally():
    try:
        raise ValueError("first")
    except ValueError:
        raise KeyError("second")
    finally:
        raise TypeError("third")


try:
    raise_in_finally()
except TypeError as e:
    chain = []
    cur = e
    while cur is not None:
        chain.append(repr(cur))
        cur = cur.__context__
    print("chain", chain)


def finally_return_swallows():
    try:
        raise ValueError
    except ValueError:
        {}["missing"]
    finally:
        return "swallowed"


print(finally_return_swallows())


def as_var_unbound_in_finally():
    try:
        raise ValueError
    except ValueError as e:
        raise KeyError
    finally:
        try:
            print(e)
        except NameError as ne:
            print("unbound:", type(ne).__name__)


try:
    as_var_unbound_in_finally()
except KeyError:
    print("KeyError propagated")


def nested_handlers_finally():
    log = []
    try:
        try:
            raise ValueError
        except ValueError:
            try:
                raise KeyError
            except KeyError:
                [][1]
            finally:
                log.append("inner fin")
        finally:
            log.append("middle fin")
    except IndexError:
        log.append("caught IndexError")
    finally:
        log.append("outer fin")
    return log


print(nested_handlers_finally())


# 16. except*: an explicit raise in the handler runs finally exactly once.
section("except star")


def star_raise():
    try:
        raise ExceptionGroup("g", [ValueError(1)])
    except* ValueError:
        raise KeyError("k")
    finally:
        print("fin")


try:
    star_raise()
except BaseException as e:
    print("caught", type(e).__name__)


def star_implicit():
    try:
        raise ExceptionGroup("g", [ValueError(1)])
    except* ValueError:
        int("x")
    finally:
        print("fin")


try:
    star_implicit()
except BaseException as e:
    print("caught", type(e).__name__)


def gen_star():
    try:
        raise ExceptionGroup("g", [ValueError(1)])
    except* ValueError:
        yield 1
    finally:
        print("fin")


it = gen_star()
next(it)
it.close()
print("closed")


# 17. Async generators and coroutines share the same handler layout.
section("async")


async def agen():
    try:
        yield 1
        raise ValueError
    except ValueError:
        await asyncio.sleep(0)
        yield 2
    finally:
        print("agen fin", exc_name())


async def drive_aclose():
    ag = agen()
    print(await ag.__anext__())
    print(await ag.__anext__())
    await ag.aclose()
    print("aclosed")

    ag = agen()
    await ag.__anext__()
    await ag.__anext__()
    try:
        await ag.athrow(KeyError("athrown"))
    except KeyError as e:
        print("caught", repr(e))


asyncio.run(drive_aclose())


async def coro():
    try:
        raise ValueError
    except ValueError:
        await Suspend()
    finally:
        print("coro fin", exc_name())


c = coro()
print(c.send(None))
c.close()

c = coro()
c.send(None)
try:
    c.throw(KeyError("into coro"))
except KeyError as e:
    print("caught", repr(e))

print("done")
