# Parity fixture: a field on a continuation line of a multi-line f-string
# reports the field's actual line in the traceback, not the f-string's start
# line (issue #2587).
#
# CPython 3.12 anchors the traceback (and the PEP 657 caret) on the physical
# line where the failing `{field}` appears.  pyrust previously stamped every
# field with the statement's start line, so a multi-line f-string reported the
# wrong line (and the caret could not appear).
#
# The parity harness strips `Traceback`/`File "..."` rows and caret underlines
# before diffing, so this fixture pins the line number explicitly: it reads the
# raising frame's `tb_lineno` out of the traceback object and prints it to
# stdout, where the harness *does* compare it byte-for-byte against CPython.

import sys


def field_line():
    """Line number of the innermost traceback frame in the current module."""
    tb = sys.exc_info()[2]
    while tb is not None and tb.tb_next is not None:
        tb = tb.tb_next
    return tb.tb_lineno


x = None


# --- field on a continuation line of an implicitly-joined f-string -----------
try:
    result = (
        f"hello "
        f"{x.missing}"  # field here
    )
except AttributeError:
    print("case1 line:", field_line())


# --- field on line 4 of a triple-quoted f-string -----------------------------
try:
    y = f"""
    first line
    {x.missing}
    last line
    """
except AttributeError:
    print("case2 line:", field_line())


# --- two fields on different physical lines: each reports its own line --------
def boom():
    raise ValueError("boom")


try:
    z = (
        f"{1 + 1}"
        f"{boom()}"
    )
except ValueError:
    print("case3 line:", field_line())


# --- single-line f-string still reports the statement line -------------------
try:
    w = f"{x.missing}"
except AttributeError:
    print("case4 line:", field_line())


# --- field on a later line inside one triple-quoted f-string with a spec ------
width = None
try:
    u = f"""line A
    {42:{width.bad}}
    line C"""
except AttributeError:
    print("case5 line:", field_line())


# --- nested f-string field on a deeper line of its own triple-quoted body -----
# The outer field's value is itself a multi-line f-string; its inner `{x.bad}`
# must anchor on the inner field's absolute line, not the outer field's line.
n = None
try:
    v = f"""
    outer
    {f'''
    inner
    {n.bad}
    '''}
    """
except AttributeError:
    print("case6 line:", field_line())


print("done")
