# Small helper calls retain real Python frames. This fixture asserts the frame
# name and line number per `__traceback__` node without letting the exception
# escape.


def walk(tb):
    rows = []
    node = tb
    while node is not None:
        rows.append((node.tb_lineno, node.tb_frame.f_code.co_name))
        node = node.tb_next
    return rows


# --- single helper: arithmetic on a bad operand ---
def add_one(x):
    return 1 + x


try:
    add_one("z")
except TypeError as e:
    # Two frames: the <module> call site, then the `add_one` body.
    print("add_one frames:", walk(e.__traceback__))


# --- helper with a subscript ---
def first(seq):
    return seq[0]


try:
    first(5)
except TypeError as e:
    print("first frames:", walk(e.__traceback__))


# --- multi-argument helper ---
def add3(a, b, c):
    return a + b + c


try:
    add3(1, "x", 3)
except TypeError as e:
    print("add3 frames:", walk(e.__traceback__))


# --- helper called from inside another function ---
def square(n):
    return n * n


def caller():
    return square("bad")


try:
    caller()
except TypeError as e:
    # Three frames: <module>, caller, square body.
    print("nested frames:", walk(e.__traceback__))


# --- caught/re-raised helper error keeps its original frames ---
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


# --- normal (non-error) calls still return correctly ---
def mul(a, b):
    return a * b


print("mul result:", mul(6, 7))
print("add_one result:", add_one(41))
