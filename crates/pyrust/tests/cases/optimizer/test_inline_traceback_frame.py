# Issue #2569: the pure-function inliner (optimizer.rs::pass_inline) splices a
# small pure leaf function's body into its call site and eliminates the call
# frame.  Before #2569 an error raised inside an inlined body lost the callee's
# traceback frame (and its PEP 657 caret), diverging from CPython 3.12.
#
# This fixture asserts the reconstructed callee frame STRUCTURE — frame name +
# line number per `__traceback__` node — which the parity harness can observe
# without letting the exception escape (it strips the stderr `File "…"` / caret
# rows, but `tb_frame.f_code.co_name` / `tb_lineno` print to stdout).


def walk(tb):
    rows = []
    node = tb
    while node is not None:
        rows.append((node.tb_lineno, node.tb_frame.f_code.co_name))
        node = node.tb_next
    return rows


# --- single inlined helper: arithmetic on a bad operand ---
def add_one(x):
    return 1 + x


try:
    add_one("z")
except TypeError as e:
    # Two frames: the <module> call site, then the inlined `add_one` body.
    print("add_one frames:", walk(e.__traceback__))


# --- inlined helper with a subscript ---
def first(seq):
    return seq[0]


try:
    first(5)
except TypeError as e:
    print("first frames:", walk(e.__traceback__))


# --- multi-argument inlined helper ---
def add3(a, b, c):
    return a + b + c


try:
    add3(1, "x", 3)
except TypeError as e:
    print("add3 frames:", walk(e.__traceback__))


# --- inlined helper called from inside another (non-inlined) function ---
def square(n):
    return n * n


def caller():
    return square("bad")


try:
    caller()
except TypeError as e:
    # Three frames: <module>, caller, inlined square body.
    print("nested frames:", walk(e.__traceback__))


# --- inlined error caught and re-raised: the original context keeps its frames ---
def doubler(v):
    return v * 2


try:
    try:
        doubler("oops")
    except TypeError:
        raise ValueError("wrapped")
except ValueError as e:
    print("wrapped class:", type(e).__name__)
    print("wrapped context frames:", walk(e.__context__.__traceback__))


# --- normal (non-error) inlined calls still return correctly ---
def mul(a, b):
    return a * b


print("mul result:", mul(6, 7))
print("add_one result:", add_one(41))
