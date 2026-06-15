# Issue #2445: an exception thrown into a generator via `.throw()` and caught
# inside the generator must attribute its traceback to the generator's own
# frame (at the suspended `yield` line), not the caller's `.throw()` call site.


def walk(tb):
    out = []
    while tb is not None:
        out.append((tb.tb_frame.f_code.co_name, tb.tb_lineno))
        tb = tb.tb_next
    return out


# 1. Caught inside the generator: tb points at the generator frame / yield line.
def gen_catch():
    try:
        yield 1
    except ValueError as e:
        print("caught:", walk(e.__traceback__))
        yield 2


g = gen_catch()
next(g)
g.throw(ValueError("x"))


# 2. Re-raised out of the generator: caller frame prepended, generator frame kept.
def gen_reraise():
    try:
        yield 1
    except ValueError:
        raise


g = gen_reraise()
next(g)
try:
    g.throw(ValueError("y"))
except ValueError as e:
    print("reraised:", walk(e.__traceback__))


# 3. Not caught at all: propagates with caller + generator frames.
def gen_passthrough():
    yield 1
    yield 2


g = gen_passthrough()
next(g)
try:
    g.throw(KeyError("z"))
except KeyError as e:
    print("uncaught:", walk(e.__traceback__))


# 4. with-statement inside the generator body: still attributed to the generator.
import contextlib


@contextlib.contextmanager
def cm():
    yield


def gen_with():
    with cm():
        try:
            yield 1
        except ValueError as e:
            print("with:", e.__traceback__.tb_frame.f_code.co_name)
            yield 2


g = gen_with()
next(g)
g.throw(ValueError("w"))


# 5. yield-from delegation: inner generator catches; tb names the inner frame.
def inner():
    try:
        yield 1
    except ValueError as e:
        print("delegated:", e.__traceback__.tb_frame.f_code.co_name)
        yield 2


def outer():
    yield from inner()


g = outer()
next(g)
print("outer-throw-result:", g.throw(ValueError("d")))
