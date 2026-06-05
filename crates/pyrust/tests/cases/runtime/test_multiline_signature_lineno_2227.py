# Issue #2227: co_firstlineno / tb_lineno must account for physical newlines
# consumed inside open brackets (implicit line continuation) and inside
# multi-line string literals, even though no NEWLINE token is emitted there.
import sys


# Multi-line def signature: co_firstlineno is the `def` line (1-based).
def f(
    a,
    b,
):
    return sys._getframe().f_code.co_firstlineno


print("multiline-sig firstlineno:", f(1, 2))


# A multi-line string before a def must not shift the def's line number.
doc = """
line 2
line 3
"""


def g():
    return sys._getframe().f_code.co_firstlineno


print("after-multiline-string firstlineno:", g())


# A bracketed multi-line expression before a def likewise must not shift it.
total = (
    1
    + 2
    + 3
)


def h():
    return sys._getframe().f_code.co_firstlineno


print("after-bracket-continuation firstlineno:", h())


# tb_lineno for an error raised through a multi-line call site reports the
# physical line of the failing statement.
def boom():
    raise ValueError("x")


try:
    boom(
    )
except ValueError:
    print("multiline-call tb_lineno:", sys.exc_info()[2].tb_lineno)


# Single-line def remains correct (no regression in baseline tracking).
def single():
    return sys._getframe().f_code.co_firstlineno


print("single-line firstlineno:", single())
