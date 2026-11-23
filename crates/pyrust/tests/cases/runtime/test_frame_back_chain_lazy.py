# Issue #3028: `sys._getframe()` resolves `f_back` lazily on demand instead of
# eagerly building and snapshotting every outer frame, while preserving full
# CPython 3.12 semantics for live chains, direct depth lookups, escaped frames,
# multi-hop unwound chains, generators, and exception unwinding.

import sys

# 1. Deep `sys._getframe().f_code.co_name` without touching `f_back`.
def deep_name(depth):
    a = depth + 1
    b = depth + 2
    c = depth + 3
    if depth == 0:
        return sys._getframe().f_code.co_name
    return deep_name(depth - 1)


print("deep_name:", deep_name(40))


# 2. Live `f_back` walk and exact identity with `sys._getframe(k)`.
def level3():
    v3 = "three"
    f0 = sys._getframe()
    f1 = sys._getframe(1)
    f2 = sys._getframe(2)
    f3 = sys._getframe(3)
    print(
        "live_identity:",
        f0.f_back is f1,
        f0.f_back.f_back is f2,
        f0.f_back.f_back.f_back is f3,
        f1.f_back is f2,
        f2.f_back is f3,
    )
    print(
        "live_names:",
        f0.f_code.co_name,
        f0.f_back.f_code.co_name,
        f0.f_back.f_back.f_code.co_name,
        f0.f_back.f_back.f_back.f_code.co_name,
    )
    print(
        "live_locals:",
        f0.f_locals["v3"],
        f0.f_back.f_locals["v2"],
        f0.f_back.f_back.f_locals["v1"],
    )


def level2():
    v2 = "two"
    level3()


def level1():
    v1 = "one"
    level2()


level1()


# 3. Direct `sys._getframe(k)` skipping intermediate frames, then reading `f_back`.
def skip_leaf():
    mid = sys._getframe(2)
    return (
        mid.f_code.co_name,
        mid.f_back.f_code.co_name,
        mid.f_back is sys._getframe(3),
    )


def skip_c():
    return skip_leaf()


def skip_b():
    return skip_c()


def skip_a():
    return skip_b()


print("skip_depth:", skip_a())


# 4. Escaped frame after return: `f_locals` and `f_back` remain intact.
def maker():
    inner = "gone"
    return sys._getframe()


def caller_of_maker():
    tag = "caller_tag"
    return maker()


escaped = caller_of_maker()
print("escaped_single:", escaped.f_code.co_name, escaped.f_locals["inner"])
print(
    "escaped_single_back:",
    escaped.f_back.f_code.co_name,
    escaped.f_back.f_locals["tag"],
    escaped.f_back.f_back.f_code.co_name,
    escaped.f_back.f_back.f_back is None,
)


# 5. Multi-hop escaped frame chain: `outer -> middle -> maker`.
def chain_maker():
    m_local = "maker_val"
    return sys._getframe()


def chain_middle():
    mid_local = "middle_val"
    return chain_maker()


def chain_outer():
    out_local = "outer_val"
    return chain_middle()


chain_f = chain_outer()
print(
    "chain_names:",
    chain_f.f_code.co_name,
    chain_f.f_back.f_code.co_name,
    chain_f.f_back.f_back.f_code.co_name,
    chain_f.f_back.f_back.f_back.f_code.co_name,
    chain_f.f_back.f_back.f_back.f_back is None,
)
print(
    "chain_locals:",
    chain_f.f_locals["m_local"],
    chain_f.f_back.f_locals["mid_local"],
    chain_f.f_back.f_back.f_locals["out_local"],
)


# 6. Generator calling `maker()`: `fg.f_back is g.gi_frame` and
#    `g.gi_frame.f_back is None` after yield.
def gen_caller():
    g_local = "gen_val"
    fg = maker()
    yield fg


g = gen_caller()
fg = next(g)
print(
    "gen_escaped:",
    fg.f_code.co_name,
    fg.f_back is g.gi_frame,
    fg.f_back.f_code.co_name,
    fg.f_back.f_locals["g_local"],
    g.gi_frame.f_back is None,
)
# Exhaust the generator; `fg.f_back` still points to the generator's frame
# whose `f_back` is `None`.
try:
    next(g)
except StopIteration:
    pass
print("gen_after_done:", fg.f_back.f_code.co_name, fg.f_back.f_back is None)


# 7. Exception unwinding: both escaped frame captured before raise and
#    non-escaped frame during unwind.
captured = []


def exc_inner(capture):
    e_in = "exc_in"
    if capture:
        captured.append(sys._getframe())
    else:
        _ = sys._getframe().f_code.co_name
    raise ValueError("boom")


def exc_outer(capture):
    e_out = "exc_out"
    exc_inner(capture)


for flag in (False, True):
    try:
        exc_outer(flag)
    except ValueError as err:
        print("exc_caught:", flag, str(err))

ef = captured[0]
print(
    "exc_escaped:",
    ef.f_code.co_name,
    ef.f_locals["e_in"],
    ef.f_back.f_code.co_name,
    ef.f_back.f_locals["e_out"],
    ef.f_back.f_back.f_code.co_name,
)
