# Pins the observable behaviour around the plain built-in exception fast
# paths (issue #3426): direct allocation of exceptions that carry only
# `BaseException` state, the stamped per-class exception classification, the
# catching frame's locals that a traceback now receives lazily, and `except`
# clauses that no longer copy an unbound exception into a register.
import sys
import traceback


def tb_lines(exc):
    out = []
    tb = exc.__traceback__
    while tb is not None:
        out.append((tb.tb_frame.f_code.co_name, tb.tb_lineno))
        tb = tb.tb_next
    return out


def visible(mapping):
    return sorted(
        (name, value)
        for name, value in mapping.items()
        if isinstance(value, (int, str))
    )


print("== construction: args, repr, str ==")
for cls in (
    BaseException,
    Exception,
    ValueError,
    TypeError,
    KeyError,
    IndexError,
    LookupError,
    ArithmeticError,
    ZeroDivisionError,
    RuntimeError,
    NotImplementedError,
    AssertionError,
    StopIteration,
    AttributeError,
    NameError,
    ImportError,
    OSError,
    SystemExit,
):
    for call_args in ((), (1,), ("msg",), (1, "two"), (1, 2, 3), ((1, 2),), ([],)):
        e = cls(*call_args)
        print(cls.__name__, e.args, repr(e), str(e), e.__traceback__, e.__context__, e.__cause__)

print("== construction: argument tuple identity and types ==")
payload = [1, 2]
e = ValueError(payload)
print(e.args[0] is payload, type(e.args) is tuple, len(e.args))
e = ValueError(payload, payload)
print(e.args[0] is e.args[1], len(e.args))
e = ValueError()
print(e.args == (), e.args is ValueError().args)
single = (7,)
e = KeyError(single)
print(e.args, e.args[0] is single, repr(e), str(e))

print("== construction: keyword arguments are rejected ==")
for cls in (ValueError, KeyError, Exception, BaseException, StopIteration):
    try:
        cls(msg="x")
    except TypeError as err:
        print(cls.__name__, "TypeError", err)
try:
    ValueError(1, 2, key=3)
except TypeError as err:
    print("mixed", err)

print("== construction: family attributes survive the fast path split ==")
print(StopIteration(5).value, StopIteration().value, StopIteration(1, 2).value)
print(SystemExit(3).code, SystemExit().code, SystemExit(1, 2).code)
print(NameError("n").name, AttributeError("a").name, AttributeError("a").obj)
print(ImportError("i").name, ImportError("i").path, ImportError("i", name="mod").name)
o = OSError(2, "gone", "file.txt")
print(type(o).__name__, o.errno, o.strerror, o.filename, o.args)
u = UnicodeDecodeError("utf-8", b"\xff", 0, 1, "bad")
print(u.encoding, u.start, u.end, u.reason)
eg = ExceptionGroup("grp", [ValueError(1), KeyError("k")])
print(eg.message, [type(x).__name__ for x in eg.exceptions], eg.args[0])

print("== construction: subclasses ==")


class Plain(ValueError):
    pass


class WithInit(ValueError):
    def __init__(self, code, detail="d"):
        super().__init__(code)
        self.code = code
        self.detail = detail


class WithNew(ValueError):
    def __new__(cls, *args):
        self = super().__new__(cls, "new", *args)
        self.made = True
        return self


class WithStr(KeyError):
    def __str__(self):
        return "custom:" + repr(self.args)


class Multi(KeyError, ValueError):
    pass


for e in (Plain(1), Plain(), WithInit(4), WithInit(4, "z"), WithNew(9), WithStr("k"), Multi("m")):
    print(type(e).__name__, e.args, repr(e), str(e), isinstance(e, ValueError))
print(WithInit(4).code, WithInit(4, "z").detail, WithNew(9).made)

print("== classification follows class mutation ==")


class Late(ValueError):
    pass


first = Late(1)
print(first.args, hasattr(first, "tag"))


def late_init(self, *args):
    ValueError.__init__(self, *args)
    self.tag = "init-ran"


Late.__init__ = late_init
second = Late(2)
print(second.args, second.tag)
del Late.__init__
third = Late(3)
print(third.args, hasattr(third, "tag"))


def base_init(self, *args):
    Exception.__init__(self, "patched", *args)


class Parent(Exception):
    pass


class Child(Parent):
    pass


print(Child(1).args)
Parent.__init__ = base_init
print(Child(1).args)
del Parent.__init__
print(Child(1).args)

print("== same-frame raise / catch ==")


def same_frame(kind):
    marker = "local-" + kind
    try:
        if kind == "class":
            raise ValueError
        if kind == "instance":
            raise ValueError(kind)
        if kind == "key":
            {}[kind]
        if kind == "index":
            [][3]
        if kind == "attr":
            marker.missing
        if kind == "zero":
            1 / 0
        if kind == "stop":
            next(iter(()))
        if kind == "int":
            int("x")
    except (ValueError, LookupError, AttributeError, ArithmeticError, StopIteration) as err:
        return type(err).__name__, err.args, tb_lines(err), marker
    return "none"


for kind in ("class", "instance", "key", "index", "attr", "zero", "stop", "int"):
    print(same_frame(kind))

print("== unbound handlers: bare, typed, tuple, nested ==")


def unbound(n):
    seen = []
    for step in range(n):
        try:
            if step % 3 == 0:
                raise ValueError(step)
            if step % 3 == 1:
                raise KeyError(step)
            raise IndexError(step)
        except ValueError:
            seen.append(("V", step, sys.exc_info()[1].args))
        except (KeyError, TypeError):
            seen.append(("K", step, repr(sys.exc_info()[1])))
        except:
            seen.append(("bare", step, type(sys.exc_info()[1]).__name__))
        seen.append(sys.exc_info()[0])
    return seen


print(unbound(6))


def unbound_reraise():
    try:
        try:
            raise ValueError("inner")
        except KeyError:
            print("unreachable")
    except ValueError as err:
        return err.args, tb_lines(err)


print(unbound_reraise())


def unbound_with_finally():
    order = []
    try:
        try:
            raise ValueError("f")
        except ValueError:
            order.append("handler")
        finally:
            order.append("finally")
    finally:
        order.append("outer")
    return order, sys.exc_info()[0]


print(unbound_with_finally())


def unbound_then_bound():
    out = []
    for err_cls in (ValueError, KeyError, ZeroDivisionError):
        try:
            raise err_cls("x")
        except ValueError:
            out.append("unbound")
        except KeyError as bound:
            out.append(("bound", bound.args))
        except Exception as other:
            out.append(("other", type(other).__name__))
    return out


print(unbound_then_bound())

print("== traceback locals: read while the catching frame is live ==")


def live_locals():
    before = 1
    try:
        raise ValueError("live")
    except ValueError as err:
        after = 2
        frame = err.__traceback__.tb_frame
        print(frame is sys._getframe(), visible(frame.f_locals))
        later = 3
        print(visible(err.__traceback__.tb_frame.f_locals))
        return visible(frame.f_locals), before, after, later


print(live_locals())

print("== traceback locals: read after the catching frame returned ==")


def escaping(tag):
    first = "f-" + tag
    try:
        raise ValueError(tag)
    except ValueError as err:
        saved = err
    second = "s-" + tag
    third = len(tag)
    return saved


exc = escaping("abc")
print(tb_lines(exc), visible(exc.__traceback__.tb_frame.f_locals))
print(visible(exc.__traceback__.tb_frame.f_locals))
print(exc.__traceback__.tb_frame.f_locals is exc.__traceback__.tb_frame.f_locals)


def escaping_many():
    kept = []
    counter = 0
    for label in ("a", "b", "c"):
        try:
            raise KeyError(label)
        except KeyError as err:
            kept.append(err)
        counter += 1
    final = "done"
    return kept


for exc in escaping_many():
    print(exc.args, tb_lines(exc), visible(exc.__traceback__.tb_frame.f_locals))


def escaping_unbound_handler():
    holder = []
    value = 10
    try:
        raise ValueError("u")
    except ValueError:
        holder.append(sys.exc_info()[1])
    value = 20
    return holder[0]


exc = escaping_unbound_handler()
print(tb_lines(exc), visible(exc.__traceback__.tb_frame.f_locals))


def escaping_with_locals_call():
    value = 1
    try:
        raise ValueError("l")
    except ValueError as err:
        saved = err
        snapshot = visible(locals())
    value = 2
    return saved, snapshot


exc, snapshot = escaping_with_locals_call()
print(snapshot, visible(exc.__traceback__.tb_frame.f_locals))


def escaping_deleted_local():
    gone = "soon"
    kept = "stay"
    try:
        raise ValueError("d")
    except ValueError as err:
        saved = err
    del gone
    return saved


exc = escaping_deleted_local()
print(visible(exc.__traceback__.tb_frame.f_locals))


def escaping_through_raise(tag):
    inner_local = "in-" + tag
    try:
        raise ValueError(tag)
    except ValueError:
        inner_local = inner_local + "!"
        raise


def outer_catch():
    outer_local = "out"
    try:
        escaping_through_raise("r")
    except ValueError as err:
        return err


exc = outer_catch()
print(tb_lines(exc))
tb = exc.__traceback__
while tb is not None:
    print(tb.tb_frame.f_code.co_name, visible(tb.tb_frame.f_locals))
    tb = tb.tb_next

print("== traceback locals: generator frames ==")


def gen_catch():
    stage = "start"
    try:
        raise ValueError("gen")
    except ValueError as err:
        saved = err
    stage = "yielded"
    yield saved
    stage = "resumed"
    yield saved


g = gen_catch()
exc = next(g)
print(tb_lines(exc), visible(exc.__traceback__.tb_frame.f_locals))
exc2 = next(g)
print(exc2 is exc, visible(exc.__traceback__.tb_frame.f_locals))

print("== chaining ==")


def chain():
    try:
        try:
            raise KeyError("first")
        except KeyError:
            raise ValueError("second")
    except ValueError as err:
        return err


exc = chain()
print(repr(exc), repr(exc.__context__), exc.__cause__, exc.__suppress_context__)
print(tb_lines(exc), tb_lines(exc.__context__))


def chain_from():
    try:
        try:
            raise KeyError("first")
        except KeyError as inner:
            raise ValueError("second") from inner
    except ValueError as err:
        return err


exc = chain_from()
print(repr(exc.__cause__), exc.__cause__ is exc.__context__, exc.__suppress_context__)


def chain_from_none():
    try:
        try:
            raise KeyError("first")
        except KeyError:
            raise ValueError("second") from None
    except ValueError as err:
        return err


exc = chain_from_none()
print(exc.__cause__, repr(exc.__context__), exc.__suppress_context__)

print("== notes ==")
try:
    raise ValueError("noted")
except ValueError as err:
    err.add_note("one")
    err.add_note("two")
    print(err.__notes__, err.args)
    print(traceback.format_exception_only(err))

print("== re-raising one instance grows its traceback ==")
shared = ValueError("shared")
for _ in range(3):
    try:
        raise shared
    except ValueError:
        pass
print(len(tb_lines(shared)), {name for name, _ in tb_lines(shared)})

print("== sys.exc_info and format_exc inside handlers ==")


def formatted():
    try:
        {}["missing"]
    except KeyError:
        kind, value, tb = sys.exc_info()
        text = traceback.format_exc()
        return kind.__name__, repr(value), tb.tb_lineno, text.splitlines()[-1], text.count("File ")


print(formatted())
print(sys.exc_info())

print("== KeyError rendering ==")
for key in ("k", 1, (1, 2), None, "", b"b", 1.5):
    try:
        {}[key]
    except KeyError as err:
        print(repr(err), str(err), err.args, err.args[0] is key)

print("== except* ==")
try:
    raise ExceptionGroup("g", [ValueError(1), KeyError("k"), TypeError("t")])
except* ValueError as group:
    print("V", [repr(x) for x in group.exceptions])
except* KeyError:
    print("K", type(sys.exc_info()[1]).__name__)
except* TypeError as group:
    print("T", group.exceptions[0].args)

print("== settrace sees the same events ==")
events = []
# Relative lines of the `try:` keywords in `traced`; their line events are
# not part of what this fixture pins.
TRY_LINES = {2, 9, 14}


def tracer(frame, event, arg):
    if frame.f_code.co_name == "traced":
        line = frame.f_lineno - frame.f_code.co_firstlineno
        if event == "exception":
            events.append((event, line, arg[0].__name__))
        elif event != "line" or line not in TRY_LINES:
            events.append((event, line))
    return tracer


def traced(flag):
    total = 0
    try:
        total += 1
        if flag:
            raise ValueError("t")
        total += 10
    except ValueError:
        total += 100
    try:
        total += 1
        {}[total]
    except KeyError as err:
        total += 1000
    try:
        total += 1
        raise IndexError
    except (IndexError, TypeError):
        total += 10000
    return total


sys.settrace(tracer)
result = traced(True)
sys.settrace(None)
print(result)
for event in events:
    print(event)
events.clear()
sys.settrace(tracer)
result = traced(False)
sys.settrace(None)
print(result)
for event in events:
    print(event)

print("== an exception crossing frames that each caught another one ==")


def deep(n):
    if n == 0:
        raise KeyError("deep")
    try:
        raise ValueError(n)
    except ValueError:
        pass
    return deep(n - 1)


try:
    deep(2)
except KeyError as err:
    print(repr(err), tb_lines(err), err.__context__)
    text = traceback.format_exc()
    print(text.count("File "), text.count("in deep"), text.splitlines()[-1])
