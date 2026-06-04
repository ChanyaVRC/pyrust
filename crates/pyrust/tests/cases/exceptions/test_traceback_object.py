# Issue #2170: e.__traceback__ is a real walkable traceback object chain,
# sys.exc_info()[2] returns it, and each node exposes
# tb_frame / tb_next / tb_lineno / tb_lasti.  Asserts STRUCTURE (frame names,
# line numbers, chain depth) — never object addresses.
import sys


def inner():
    raise ValueError("boom")


def outer():
    inner()


try:
    outer()
except ValueError as e:
    tb = e.__traceback__
    # The whole (type, value, traceback) tuple is populated.
    info = sys.exc_info()
    print("exc_info populated:", [x is not None for x in info])
    print("exc_info tb is __traceback__:", info[2] is tb)
    # The chain walks outermost -> innermost via tb_next.
    rows = []
    node = tb
    while node is not None:
        rows.append((node.tb_lineno, node.tb_frame.f_code.co_name))
        node = node.tb_next
    print("depth:", len(rows))
    for lineno, name in rows:
        print(lineno, name)
    # tb_lasti is an int (best-effort; pyrust uses -1).
    print("tb_lasti is int:", isinstance(tb.tb_lasti, int))
    print("tb_frame type:", type(tb.tb_frame).__name__)
    print("tb type:", type(tb).__name__)


# A freshly-constructed exception still has __traceback__ == None.
print("fresh tb:", RuntimeError("x").__traceback__)

# Outside any handler, exc_info() is all-None.
print("outside handler:", [x is None for x in sys.exc_info()])
