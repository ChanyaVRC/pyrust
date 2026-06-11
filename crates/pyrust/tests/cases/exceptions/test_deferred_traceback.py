# Issue #2351: the `__traceback__` slot of a caught exception is built lazily
# (a cheap placeholder at catch time, materialised on first read) instead of
# eagerly.  This must be byte-identical to the old eager construction.  Covers
# traceback walking, identity stability, sys.exc_info integration, explicit and
# implicit chaining, bare re-raise, subclass/tuple matching, finally control
# flow, custom __init__ overrides, and generator throw/close/escape.

import sys


# Deferred __traceback__ walks the full unwound frame chain (outermost first).
def inner():
    raise ValueError("boom")


def middle():
    inner()


def outer():
    middle()


try:
    outer()
except ValueError as e:
    tb = e.__traceback__
    names = []
    while tb is not None:
        names.append(tb.tb_frame.f_code.co_name)
        names.append(tb.tb_lineno)
        tb = tb.tb_next
    print(names)
    print(type(e.__traceback__).__name__)
    # Repeated reads return the same materialised object.
    print(e.__traceback__ is e.__traceback__)
    # sys.exc_info()'s traceback is the same object as e.__traceback__.
    et, ev, etb = sys.exc_info()
    print(ev is e, etb is e.__traceback__)

# Fresh (unraised) exception has __traceback__ == None.
print(ValueError("x").__traceback__ is None)


# Subclass / tuple except matching, with traceback present.
class MyErr(ValueError):
    pass


try:
    raise MyErr("sub")
except (KeyError, ValueError) as e:
    print(type(e).__name__, e.__traceback__ is not None)

# raise X from Y: cause/context/suppress and both tracebacks materialise.
try:
    try:
        raise KeyError("k")
    except KeyError as inner_exc:
        raise ValueError("v") from inner_exc
except ValueError as e:
    print(type(e.__cause__).__name__, type(e.__context__).__name__)
    print(e.__suppress_context__)
    print(e.__traceback__ is not None, e.__cause__.__traceback__ is not None)

# Implicit chaining (raise inside except, no `from`).
try:
    try:
        raise KeyError("k")
    except KeyError:
        raise ValueError("v2")
except ValueError as e:
    print(type(e.__context__).__name__, e.__suppress_context__)


# Bare `raise` re-raise across a function boundary.
def reraise():
    try:
        raise IndexError("idx")
    except IndexError:
        raise


try:
    reraise()
except IndexError as e:
    print(type(e).__name__, e.__traceback__ is not None)


# finally with return / break.
def fin_return():
    try:
        return "try-value"
    finally:
        print("finally-ran")


print(fin_return())


def fin_break():
    out = []
    for i in range(3):
        try:
            if i == 1:
                break
        finally:
            out.append(i)
    return out


print(fin_break())


# Custom __init__ override; traceback still attaches.
class Custom(Exception):
    def __init__(self, x, y):
        super().__init__(f"{x}-{y}")
        self.x = x


try:
    raise Custom(1, 2)
except Custom as e:
    print(str(e), e.x, e.__traceback__ is not None)


# Generator throw: the exception is caught inside the generator, whose
# __traceback__ materialises correctly.
def gen():
    try:
        yield 1
        yield 2
    except ValueError as ge:
        print("gen-caught", str(ge), ge.__traceback__ is not None)
        yield 99
    finally:
        print("gen-finally")


g = gen()
print(next(g))
print(g.throw(ValueError("thrown")))
try:
    next(g)
except StopIteration:
    print("gen-stopped")


# Exception escaping a generator body materialises its traceback in the
# consumer.
def gen_escape():
    yield 1
    raise RuntimeError("escape")


ge = gen_escape()
print(next(ge))
try:
    next(ge)
except RuntimeError as e:
    print("escaped", str(e), e.__traceback__ is not None)
