# PEP 657 caret anchors on *generator* frames (issue #2904).
#
# A generator frame's traceback entry used to be recorded with no column anchor
# at all, so every generator frame rendered caret-free — `yield 1/0` printed the
# right source line but never the `~^~` CPython 3.12 draws under `1/0`, and
# `yield f()` never drew the `^^^` under `f()`.  Both generator-frame recording
# sites (the `for`-loop gen-drive trampoline and the plain
# `next()` / `.send()` / `.throw()` resume) now publish the anchor of the
# instruction that propagated the error inside the body, exactly like a plain
# function frame does.
#
# NOTE: the parity harness strips `File "..."` header rows *and* the `^`/`~`
# underline rows before diffing, so this fixture cannot pin the caret columns
# directly.  It pins the structural half — every frame's `co_name` + `tb_lineno`
# and the exception class/message — which is what the caret anchor is derived
# from; the exact caret columns are asserted byte-for-byte against CPython 3.12
# in `tests/uncaught_traceback_source_render.rs`.


def walk(exc):
    tb = exc.__traceback__
    frames = []
    while tb is not None:
        frames.append((tb.tb_frame.f_code.co_name, tb.tb_lineno))
        tb = tb.tb_next
    return frames


def raiser():
    raise ValueError("boom")


# --- `yield <binop>` raising inside the body: anchor is the `1/0` BinOp ---
def gen_div():
    yield 1 / 0


try:
    for _ in gen_div():
        pass
except ZeroDivisionError as e:
    print("yield binop:", type(e).__name__, e, walk(e))


# --- `yield <call>`: anchor is the `raiser()` Call in the generator body ---
def gen_call():
    yield raiser()


try:
    for _ in gen_call():
        pass
except ValueError as e:
    print("yield call:", type(e).__name__, e, walk(e))


# --- `x = yield <call>`: same anchor, narrower than the whole line ---
def gen_assign_call():
    x = yield raiser()
    return x


try:
    for _ in gen_assign_call():
        pass
except ValueError as e:
    print("assign yield call:", type(e).__name__, e, walk(e))


# --- `yield from <call>` raising *at the call*, before delegation starts ---
def gen_yield_from_call():
    yield from raiser()


try:
    for _ in gen_yield_from_call():
        pass
except ValueError as e:
    print("yield from call:", type(e).__name__, e, walk(e))


# --- `yield <binop with call>`: the Call is the raising instruction ---
def gen_binop_call():
    yield 1 + raiser()


try:
    for _ in gen_binop_call():
        pass
except ValueError as e:
    print("yield binop call:", type(e).__name__, e, walk(e))


# --- a plain statement before the first yield still anchors correctly ---
def gen_stmt_before_yield():
    x = 1 / 0
    yield x


try:
    for _ in gen_stmt_before_yield():
        pass
except ZeroDivisionError as e:
    print("stmt before yield:", type(e).__name__, e, walk(e))


# --- the non-trampolined resume paths: `next()` and `.send()` ---
try:
    next(gen_div())
except ZeroDivisionError as e:
    print("next():", type(e).__name__, e, walk(e))

try:
    gen_call().send(None)
except ValueError as e:
    print("send():", type(e).__name__, e, walk(e))


# --- delegation: the outer `yield from` frame and the inner generator frame
#     each get their own entry (CPython leaves the outer one caret-free because
#     `yield from inner()` is a whole-line anchor) ---
def gen_inner():
    yield raiser()


def gen_outer():
    yield from gen_inner()


try:
    for _ in gen_outer():
        pass
except ValueError as e:
    print("yield from delegation:", type(e).__name__, e, walk(e))


# --- a deeper call chain below the generator body ---
def deep_a():
    raise ValueError("deep")


def deep_b():
    return 10 * deep_a()


def gen_deep():
    yield deep_b()


try:
    for _ in gen_deep():
        pass
except ValueError as e:
    print("deep chain:", type(e).__name__, e, walk(e))


# --- `throw()` into a suspended generator whose body already raised-and-caught
#     an exception earlier: the injected exception escapes at the `yield`, which
#     carries no anchor, so the frame must NOT inherit the stale `1/0` span ---
def gen_caught_then_suspend():
    try:
        1 / 0
    except ZeroDivisionError:
        pass
    yield 1
    yield 2


g = gen_caught_then_suspend()
next(g)
try:
    g.throw(ValueError("thrown"))
except ValueError as e:
    print("throw after caught:", type(e).__name__, e, walk(e))


# --- generator expressions are generator frames too ---
try:
    list(raiser() for _ in [1])
except ValueError as e:
    print("genexpr call:", type(e).__name__, e, walk(e))
