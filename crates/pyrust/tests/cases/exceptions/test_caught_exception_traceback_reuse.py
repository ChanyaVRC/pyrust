# Pins what a frame that catches many exceptions must keep distinct
# (issue #3426).  The interpreter now reuses one internal traceback
# placeholder per activation once the previous exception is gone, lends the
# catching frame's locals lazily, and recycles exception storage; none of
# that may leak one exception's traceback, locals, arguments, or lifetime
# into another.
import sys
import traceback


def tb_lines(exc):
    out = []
    tb = exc.__traceback__
    while tb is not None:
        out.append((tb.tb_frame.f_code.co_name, tb.tb_lineno))
        tb = tb.tb_next
    return out


print("== distinct raise lines, every exception kept ==")


def all_kept():
    kept = []
    for step in range(3):
        try:
            if step == 0:
                raise ValueError(step)
            elif step == 1:
                raise KeyError(step)
            else:
                raise IndexError(step)
        except Exception as err:
            kept.append(err)
    return kept


for exc in all_kept():
    print(type(exc).__name__, exc.args, tb_lines(exc))

print("== distinct raise lines, every other exception kept ==")


def alternate():
    kept = []
    for step in range(8):
        try:
            if step % 3 == 0:
                raise ValueError(step)
            elif step % 3 == 1:
                raise KeyError(step)
            else:
                raise IndexError(step)
        except Exception as err:
            if step % 2 == 0:
                kept.append(err)
    return kept


for exc in alternate():
    print(type(exc).__name__, exc.args, tb_lines(exc))

print("== kept between dropped ==")


def kept_between_dropped():
    try:
        raise ValueError("dropped")
    except ValueError:
        pass
    try:
        raise KeyError("kept")
    except KeyError as err:
        saved = err
    try:
        raise IndexError("dropped too")
    except IndexError:
        pass
    try:
        [][1]
    except IndexError:
        pass
    return saved


exc = kept_between_dropped()
print(repr(exc), tb_lines(exc))
print(sorted(exc.__traceback__.tb_frame.f_locals))

print("== module level ==")
module_kept = []
for step in range(6):
    try:
        if step % 2:
            raise ValueError(step)
        raise KeyError(step)
    except (ValueError, KeyError) as err:
        if step < 2:
            module_kept.append(err)
for exc in module_kept:
    tb = exc.__traceback__
    print(repr(exc), tb.tb_lineno, tb.tb_next, tb.tb_frame is sys._getframe())
    print(tb.tb_frame.f_locals is globals())

print("== traceback read inside the handler, then the next catch ==")


def read_then_catch_again():
    seen = []
    for step in range(4):
        try:
            if step % 2:
                raise ValueError(step)
            raise KeyError(step)
        except (ValueError, KeyError) as err:
            if step < 2:
                seen.append((err.__traceback__.tb_lineno, err.__traceback__.tb_next))
            else:
                seen.append(tb_lines(err))
    return seen


print(read_then_catch_again())

print("== nested handlers in one frame ==")


def nested():
    out = []
    try:
        try:
            raise ValueError("inner")
        except ValueError as inner:
            out.append(tb_lines(inner))
            try:
                raise KeyError("second")
            except KeyError as second:
                out.append((tb_lines(second), repr(second.__context__)))
            try:
                raise IndexError("third")
            except IndexError:
                out.append(repr(sys.exc_info()[1]))
            raise
    except ValueError as outer:
        out.append(tb_lines(outer))
        return outer, out


exc, out = nested()
print(out)
print(tb_lines(exc), exc.__context__)

print("== re-raise through frames, alternating ==")


def inner_frame(n):
    try:
        raise ValueError(n)
    except ValueError:
        if n % 2:
            raise
    return n


def outer_frame():
    got = []
    for n in range(5):
        try:
            got.append(inner_frame(n))
        except ValueError as err:
            got.append((err.args, tb_lines(err)))
    return got


print(outer_frame())

print("== raised in a callee, caught here ==")


def thrower(n):
    local = "callee-%d" % n
    if n % 2:
        raise KeyError(n)
    raise ValueError(n)


def catch_from_callee():
    kept = []
    for n in range(4):
        try:
            thrower(n)
        except (KeyError, ValueError) as err:
            if n in (1, 2):
                kept.append(err)
    return kept


for exc in catch_from_callee():
    print(repr(exc), tb_lines(exc), exc.__traceback__.tb_next.tb_frame.f_locals)

print("== finalizer timing ==")


class Tracked:
    def __init__(self, n):
        self.n = n

    def __del__(self):
        print("del", self.n)


def finalize_after_handler():
    x = Tracked(1)
    try:
        raise ValueError
    except ValueError:
        pass
    print("before rebind")
    x = None
    print("after rebind")


finalize_after_handler()


def finalize_exception_payload():
    for n in (2, 3):
        try:
            raise ValueError(Tracked(n))
        except ValueError:
            print("in handler", n)
        print("after handler", n)


finalize_exception_payload()
print("after call")


def finalize_bound():
    try:
        raise ValueError(Tracked(4))
    except ValueError as err:
        print("in handler", err.args[0].n)
    print("after handler")


finalize_bound()
print("after call")


def finalize_native_error():
    key = Tracked(5)
    holder = {}
    try:
        holder[id(key), "missing"]
    except KeyError:
        print("caught")
    key = None
    print("after rebind")


finalize_native_error()

print("== generators ==")


def gen():
    for n in range(4):
        try:
            if n % 2:
                raise ValueError(n)
            raise KeyError(n)
        except (ValueError, KeyError) as err:
            keep = err if n % 2 else None
        yield keep


for item in gen():
    print(item if item is None else (item.args, [line for _, line in tb_lines(item)]))

print("== formatted after the frame is gone ==")


def escaper():
    local_a = 1
    try:
        {}["k"]
    except KeyError as err:
        local_b = 2
        return err


exc = escaper()
text = "".join(traceback.format_exception(exc))
print(text.count("File "), text.splitlines()[-1])
print(sorted(exc.__traceback__.tb_frame.f_locals))

print("== storage reuse keeps arguments apart ==")


def storage():
    out = []
    previous = None
    for n in range(6):
        try:
            if n % 3 == 0:
                raise ValueError(n, "two")
            if n % 3 == 1:
                raise StopIteration(n)
            raise AttributeError("attr-%d" % n)
        except Exception as err:
            out.append((type(err).__name__, err.args, err.__traceback__ is not None))
            if n == 2:
                previous = err
    return out, previous.args, previous.name, previous.obj


print(storage())
