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


# Issue #2405: a BARE `raise` re-raising an exception that already carries a
# traceback keeps the carried chain unchanged and only prepends genuinely-outer
# frames the exception unwinds through after the re-raise — it never adds a
# traceback node for the re-raising frame itself (so the carried head line, not
# the bare-`raise` line, stays at the chain head).


def bare_g():
    try:
        f()  # original raise propagation line (carried head)
    except IndexError:
        raise  # bare re-raise — NOT a new tb node


def bare_reraise_up_one():
    # Bare re-raise propagating up one frame: head stays the carried `f()` line
    # in bare_g, prefixed by this frame's call line — no bare-`raise` node.
    try:
        bare_g()
    except IndexError as exc:
        print("bare-up1:", walk(exc.__traceback__))


def bare_h():
    bare_g()


def bare_reraise_up_two():
    # Two frames of propagation above the bare re-raise.
    try:
        bare_h()
    except IndexError as exc:
        print("bare-up2:", walk(exc.__traceback__))


def bare_helper():
    raise  # bare re-raise inside a helper called from an except block


def bare_g_via_helper():
    try:
        f()
    except IndexError:
        bare_helper()


def bare_reraise_in_helper():
    # Bare re-raise inside a helper: the helper frame gets NO traceback node;
    # the chain head is the helper-call line in the except block.
    try:
        bare_g_via_helper()
    except IndexError as exc:
        print("bare-helper:", walk(exc.__traceback__))


def bare_g_same_frame():
    # Bare re-raise caught in the SAME frame: the carried chain is unchanged.
    try:
        f()
    except IndexError:
        try:
            raise
        except IndexError as e:
            print("bare-same-frame:", walk(e.__traceback__))


def bare_g_nested():
    # Nested except + two bare re-raises, then caught one frame up: only the
    # original `f()` line survives at the carried head.
    try:
        try:
            f()
        except IndexError:
            raise
    except IndexError:
        raise


def bare_reraise_nested():
    try:
        bare_g_nested()
    except IndexError as exc:
        print("bare-nested:", walk(exc.__traceback__))


with_traceback_same_frame()
raise_e_same_frame()
carried_across_frames()
reraise_in_other_fn()
tail_identity()
bare_reraise_up_one()
bare_reraise_up_two()
bare_reraise_in_helper()
bare_g_same_frame()
bare_reraise_nested()
