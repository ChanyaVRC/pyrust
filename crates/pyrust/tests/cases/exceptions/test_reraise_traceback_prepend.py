# Issue #2367: re-raising an exception that already carries a traceback
# (`raise e`, `raise e.with_traceback(tb)`, or re-raising a variable carried
# across frames) PREPENDS a node for the re-raising frame and keeps the old
# chain as the tail (same objects).  pyrust previously rebuilt the chain from
# the captured unwind frames only, dropping the re-raising frame's node (and,
# for the carried-across-frames case, the whole carried tail).
#
# Each scenario lives in its own function so the catching frame's current-line
# state is independent (sequential same-frame module-scope re-raises trip a
# separate, pre-existing current-line staleness unrelated to this fix).


def walk(tb):
    names = []
    cur = tb
    while cur is not None:
        names.append((cur.tb_frame.f_code.co_name, cur.tb_lineno))
        cur = cur.tb_next
    return names


def f():
    raise IndexError("idx")


def with_traceback_same_frame():
    # raise e.with_traceback(e.__traceback__) prepends the re-raise line.
    try:
        try:
            f()
        except IndexError as e:
            raise e.with_traceback(e.__traceback__)
    except IndexError as e:
        print("with_traceback:", walk(e.__traceback__))


def raise_e_same_frame():
    # Bare `raise e` (no with_traceback) prepends the same way.
    try:
        try:
            f()
        except IndexError as e:
            raise e
    except IndexError as e:
        print("raise-e:", walk(e.__traceback__))


def g():
    try:
        f()
    except IndexError as e:
        return e


def carried_across_frames():
    # A variable carried out of g() and re-raised: the carried (g -> f) chain
    # stays as the tail, the re-raise frame prepends.
    carried = g()
    try:
        raise carried
    except IndexError as exc:
        print("carried:", walk(exc.__traceback__))


def h(e):
    raise e


def reraise_in_other_fn():
    # Re-raise inside a different function: that frame is prepended above the
    # carried chain.
    carried = g()
    try:
        h(carried)
    except IndexError as exc:
        print("reraise-in-fn:", walk(exc.__traceback__))


def tail_identity():
    # CPython guarantees the re-raised exception's new tb head is a fresh object
    # whose tb_next IS the original (carried) tb object.
    carried = g()
    saved = carried.__traceback__
    try:
        raise carried
    except IndexError as exc:
        new_tb = exc.__traceback__
        print("head-is-saved:", new_tb is saved)
        print("tb_next-is-saved:", new_tb.tb_next is saved)


with_traceback_same_frame()
raise_e_same_frame()
carried_across_frames()
reraise_in_other_fn()
tail_identity()
