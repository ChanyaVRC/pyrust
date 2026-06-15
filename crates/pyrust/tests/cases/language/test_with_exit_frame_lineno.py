# Issue #2419: when a context manager's __exit__ raises while an exception is
# unwinding out of the `with` body, the enclosing frame's traceback node must be
# attributed to the `with` statement header line, not the body line that raised.


def walk(tb):
    rows = []
    n = tb
    while n is not None:
        rows.append((n.tb_lineno, n.tb_frame.f_code.co_name))
        n = n.tb_next
    return rows


class CM:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        raise ValueError("from exit")  # line 23


def f():
    with CM():  # line 27
        raise KeyError("orig")  # line 28


try:
    f()  # line 32
except ValueError as e:
    print(walk(e.__traceback__))


# __exit__ does not raise: the original exception keeps its own body line.
class CMNoRaise:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


def g():
    with CMNoRaise():
        raise KeyError("orig")  # line 48


try:
    g()
except KeyError as e:
    print(walk(e.__traceback__))


# Nested with: each frame shows its own header line; inner __exit__ raises.
class Outer:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        pass


class Inner:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        raise ValueError("inner exit")  # line 71


def h():
    with Outer():  # line 75
        with Inner():  # line 76
            raise KeyError("o")  # line 77


try:
    h()
except ValueError as e:
    print(walk(e.__traceback__))


# __exit__ suppressing the exception still works.
class CMSuppress:
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return True


def k():
    with CMSuppress():
        raise KeyError("orig")
    return "suppressed"


print(k())
