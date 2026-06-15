# Issue #2404: the uncaught (top-level) stderr traceback formatter must walk the
# exception's `__traceback__` chain, not the captured-frame thread-local
# snapshot.  After #2367/#2403 made `__traceback__` correctly prepend the
# re-raising frame, the snapshot path diverged: an uncaught same-frame `raise e`
# dropped the re-raise frame's node, so its echoed source line went missing from
# the stderr traceback.
#
# A genuinely-uncaught raise can't be a parity fixture directly: it exits the
# process non-zero (the harness requires status 0) and its stderr `File "..."`
# lines carry absolute paths the harness strips.  Instead this fixture walks the
# same `__traceback__` chain the stderr formatter now consumes and prints, per
# frame, the (co_name, tb_lineno, source-line text) triple -- the source-line
# text being exactly what the formatter echoes under each frame (#2428).  If the
# formatter walked the wrong chain (the old snapshot path), the re-raise frame's
# line would be absent here.


def show_frames(label, tb):
    cur = tb
    rows = []
    while cur is not None:
        co = cur.tb_frame.f_code
        with open(co.co_filename) as fh:
            line = fh.readlines()[cur.tb_lineno - 1].strip()
        rows.append((co.co_name, cur.tb_lineno, line))
        cur = cur.tb_next
    print(label)
    for name, lineno, line in rows:
        print("   ", name, lineno, repr(line))


def f():
    raise IndexError("idx")


def g_explicit():
    try:
        f()
    except IndexError as e:
        raise e  # explicit same-frame re-raise (the #2404 frame)


def g_bare():
    try:
        f()
    except IndexError:
        raise  # bare re-raise


def h_explicit(exc):
    raise exc  # re-raise in a different frame


# Explicit same-frame re-raise: the `raise e` frame must appear between the
# caller's `g_explicit()` line and the original `f()` line.
try:
    g_explicit()
except IndexError as e:
    show_frames("explicit:", e.__traceback__)

# Bare re-raise: no node for the bare `raise` itself; the carried `f()` line
# stays at the chain head.
try:
    g_bare()
except IndexError as e:
    show_frames("bare:", e.__traceback__)

# First-time raise (no prior __traceback__): plain two-frame chain.
try:
    f()
except IndexError as e:
    show_frames("first-time:", e.__traceback__)

# Re-raise carried into another frame: that frame's `raise exc` line prepends.
try:
    try:
        f()
    except IndexError as e:
        carried = e
    h_explicit(carried)
except IndexError as e:
    show_frames("reraise-other-frame:", e.__traceback__)
