# Issue #2420: when one frame raises twice (except A: raise B on line X;
# except B: raise C on line Y), the FINAL exception's traceback frame must show
# its OWN raise line (Y), not the first raise's line (X).
#
# Root cause: the optimizer's cross-jump (tail-merge) pass collapsed the two
# `raise EXC(arg)` sites — which share an identical `Call; RaiseValue` tail —
# into one survivor copy carrying only the first site's line.  The fix forbids
# merging tails whose source lines differ, so each raise keeps its own line.
#
# Asserts STRUCTURE only (frame names + tb line numbers via tb walking) — never
# caret/source-line rendering, which diverges independently (#2426/#2428).
#
# NOTE: each multi-raise scenario lives in its OWN function frame.  A separate,
# pre-existing module-frame defect (greedy `remap_linenos` mis-attributing two
# structurally-identical module-level raises — the #1962 class for raises) is
# tracked separately and is out of scope here, so the module frame holds only
# the single canonical repro.


def walk(tb):
    rows = []
    node = tb
    while node is not None:
        rows.append((node.tb_lineno, node.tb_frame.f_code.co_name))
        node = node.tb_next
    return rows


def boom():
    raise KeyError("inner")


# --- the issue's 3-deep repro, in a function frame (caught-walk) ---
def three_deep():
    try:
        try:
            boom()
        except KeyError:
            raise TypeError("mid")  # line 40
    except TypeError:
        try:
            raise ValueError("top")  # line 43 — the final raise's OWN line
        except ValueError as e:
            # ValueError node = line 43, NOT line 40.
            print("3-deep top:", walk(e.__traceback__))
            # Its __context__ (TypeError) node = line 40.
            print("3-deep ctx:", walk(e.__context__.__traceback__))


three_deep()


# --- 4-deep: two same-frame raise pairs chained in one function frame ---
def four_deep():
    try:
        try:
            try:
                boom()
            except KeyError:
                raise TypeError("a")  # line 60
        except TypeError:
            raise ValueError("b")  # line 62
    except ValueError:
        try:
            raise IndexError("c")  # line 65 — last raise's OWN line
        except IndexError as e:
            print("4-deep last:", walk(e.__traceback__))
            print("4-deep ctx1:", walk(e.__context__.__traceback__))
            print("4-deep ctx2:", walk(e.__context__.__context__.__traceback__))


four_deep()


# --- the canonical module-frame repro (single block, single ValueError) ---
try:
    try:
        boom()
    except KeyError:
        raise TypeError("mid")  # line 79
except TypeError:
    try:
        raise ValueError("top")  # line 82 — final raise's OWN line
    except ValueError as e:
        print("module top:", walk(e.__traceback__))
