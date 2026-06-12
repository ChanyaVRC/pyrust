# Issue #2407: a *fresh* exception (no carried `__traceback__`) raised inside an
# `except` / `finally` handler must NOT inherit the stale captured-frame snapshot
# of the exception it was handling.  Before the fix, the new exception's
# `__traceback__` chain picked up a spurious trailing frame — the innermost
# frame of the in-flight (handled) exception.
#
# Asserts STRUCTURE only (frame names + line numbers) — never caret/source-line
# rendering, which diverges independently (#2411).


def walk(tb):
    rows = []
    node = tb
    while node is not None:
        rows.append((node.tb_lineno, node.tb_frame.f_code.co_name))
        node = node.tb_next
    return rows


# --- fresh raise in except, handled exception unwound through a function ---
def f1():
    raise IndexError("idx")


def g1():
    try:
        f1()
    except IndexError:
        raise ValueError("v")


try:
    g1()
except ValueError as e:
    # Only g1's raise frame + the module call site — NOT f1.
    print("func except walk:", walk(e.__traceback__))


# --- fresh raise in except, same module frame ---
def boom2():
    raise KeyError("inner")


try:
    boom2()
except KeyError:
    try:
        raise ValueError("outer")
    except ValueError as e:
        print("same-frame walk:", walk(e.__traceback__))


# --- nested two excepts: each fresh raise starts clean ---
def a3():
    raise KeyError("k")


def b3():
    raise IndexError("i")


try:
    try:
        a3()
    except KeyError:
        try:
            b3()
        except IndexError:
            raise ValueError("final")
except ValueError as e:
    print("nested walk:", walk(e.__traceback__))


# --- fresh raise in a finally during unwind ---
def f4():
    raise KeyError("k")


def g4():
    try:
        f4()
    finally:
        raise ValueError("from finally")


try:
    g4()
except ValueError as e:
    print("finally walk:", walk(e.__traceback__))


# --- the new exception still records its OWN deeper unwind frames ---
def deep_inner():
    raise ValueError("deep")


def deep_outer():
    deep_inner()


def h5():
    raise KeyError("k")


try:
    h5()
except KeyError:
    try:
        deep_outer()
    except ValueError as e:
        # deep_outer + deep_inner frames present; h5 (the handled KeyError's
        # frame) absent.
        print("deep walk:", walk(e.__traceback__))
