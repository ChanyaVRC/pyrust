# Parity regression for #289: the compiler rewrites the manual-indexed
# iteration pattern `while i < len(c): use(c[i]); i += 1` into a `for x in c:`
# loop when `i` is unused after the loop and only appears as `c[i]` inside.
#
# Each case prints something Python and PyRust must agree on byte-for-byte
# under the parity_compare harness.

# (1) Canonical shape: sum elements via index then discard `i`.
def sum_indexed():
    xs = [1, 2, 3, 4, 5]
    total = 0
    i = 0
    while i < len(xs):
        total += xs[i]
        i += 1
    return total
assert sum_indexed() == 15
print("while-index-1", sum_indexed())

# (2) Empty collection — the for-loop form must skip the body entirely.
def empty_coll():
    xs = []
    seen = 0
    i = 0
    while i < len(xs):
        seen += 1
        i += 1
    return seen
assert empty_coll() == 0
print("while-index-2", empty_coll())

# (3) Single-element collection.
def single_elem():
    xs = [42]
    out = []
    i = 0
    while i < len(xs):
        out.append(xs[i])
        i += 1
    return out
assert single_elem() == [42]
print("while-index-3", single_elem())

# (4) Body that uses `c[i]` in multiple positions inside one statement.
def multi_use():
    xs = [10, 20, 30]
    total = 0
    i = 0
    while i < len(xs):
        total += xs[i] * xs[i]
        i += 1
    return total
assert multi_use() == 100 + 400 + 900
print("while-index-4", multi_use())

# (5) Bail-out: `i` is read after the loop in the same block — rewrite must
#     NOT fire, otherwise `i` would equal the last element value not len(xs).
def post_use():
    xs = [10, 20, 30]
    total = 0
    i = 0
    while i < len(xs):
        total += xs[i]
        i += 1
    # i must be exactly len(xs) here, not xs[-1].
    return (i, total)
assert post_use() == (3, 60), post_use()
print("while-index-5", post_use())

# (6) Bail-out: body reads `i` as a bare name (not via `c[i]`).  Print must
#     show indices 0, 1, 2 — not the element values.
def bare_i_read():
    xs = ["a", "b", "c"]
    out = []
    i = 0
    while i < len(xs):
        out.append((i, xs[i]))
        i += 1
    return out
assert bare_i_read() == [(0, "a"), (1, "b"), (2, "c")], bare_i_read()
print("while-index-6", bare_i_read())

# (7) Bail-out: `c[i] = ...` index-assign inside body would not behave the
#     same under a for-iter (the for-loop reads the snapshot value).  Make
#     sure mutation is preserved.
def mutate_c_at_i():
    xs = [1, 2, 3, 4]
    i = 0
    while i < len(xs):
        if xs[i] == 2:
            xs[i] = 99
        i += 1
    return xs
assert mutate_c_at_i() == [1, 99, 3, 4]
print("while-index-7", mutate_c_at_i())

# (8) Bail-out: `break` inside body — must exit at first match.
def break_inside():
    xs = [10, 20, 30, 40]
    total = 0
    i = 0
    while i < len(xs):
        if xs[i] == 30:
            break
        total += xs[i]
        i += 1
    return total
assert break_inside() == 30
print("while-index-8", break_inside())

# (9) Bail-out: `i += 2` — non-unit step.  Original semantics: visit every
#     second element.  Rewrite must not fire.
def step_two():
    xs = [1, 2, 3, 4, 5]
    out = []
    i = 0
    while i < len(xs):
        out.append(xs[i])
        i += 2
    return out
assert step_two() == [1, 3, 5]
print("while-index-9", step_two())

# (10) Function-scope rewrite: side-effecting body must run exactly len(xs)
#      times.  Counts a global to verify iteration count.
side_effect_count = 0
def with_side_effect(xs):
    global side_effect_count
    i = 0
    while i < len(xs):
        side_effect_count += 1
        _ = xs[i]
        i += 1
    return side_effect_count
side_effect_count = 0
n = with_side_effect([0] * 7)
assert n == 7
print("while-index-10", n)

# (11) Nested rewrite: inner pattern inside an outer for-loop body.
def nested():
    out = []
    for k in range(3):
        i = 0
        ys = [k, k + 1, k + 2]
        while i < len(ys):
            out.append(ys[i])
            i += 1
    return out
assert nested() == [0, 1, 2, 1, 2, 3, 2, 3, 4]
print("while-index-11", nested())

# (12) Bail-out: a nested function reads `i` — even though it's not invoked,
#      the def references it, so the rewrite must NOT fire.
def closure_reads_i():
    xs = [10, 20, 30]
    i = 0
    out = []
    while i < len(xs):
        def remember():
            return i  # captures the index, not the element
        out.append(remember())
        i += 1
    return out
# Each closure captures the *current* i at iteration time (i is a free var
# bound to the function's local i).  We print to verify behaviour stays the
# same as CPython.
print("while-index-12", closure_reads_i())

# (13) Bail-out: `i` is written elsewhere in body (extra `i += 0` is still an
#      assignment, but use a clearer case: conditional `i = something`).
def conditional_i_reassign():
    xs = [1, 2, 3, 4, 5]
    i = 0
    total = 0
    while i < len(xs):
        total += xs[i]
        if total >= 6:
            i = len(xs)  # early-exit by jumping i past end
        else:
            i += 1
    return total
assert conditional_i_reassign() == 6
print("while-index-13", conditional_i_reassign())

# (14) Iteration over a name that's still bound after the loop (no rewrite
#      reads from it) — verify the for-loop reads the same name.
def read_c_after():
    xs = [1, 2, 3]
    i = 0
    total = 0
    while i < len(xs):
        total += xs[i]
        i += 1
    return (len(xs), total)
assert read_c_after() == (3, 6)
print("while-index-14", read_c_after())

# (15) Tuple iteration: rewrite should also work for tuples.
def tuple_iter():
    t = (10, 20, 30)
    total = 0
    i = 0
    while i < len(t):
        total += t[i]
        i += 1
    return total
assert tuple_iter() == 60
print("while-index-15", tuple_iter())

# (16) String iteration: `len(s)` and `s[i]` should also rewrite cleanly.
def string_iter():
    s = "abc"
    out = []
    i = 0
    while i < len(s):
        out.append(s[i])
        i += 1
    return out
assert string_iter() == ["a", "b", "c"]
print("while-index-16", string_iter())

# (17) Assignment-form increment: `i = i + 1` (instead of `i += 1`) should
#      also be recognised.
def assign_form_inc():
    xs = [1, 2, 3]
    total = 0
    i = 0
    while i < len(xs):
        total += xs[i]
        i = i + 1
    return total
assert assign_form_inc() == 6
print("while-index-17", assign_form_inc())

# (18) Bail-out: `while-else` — Python's else runs when the loop exits
#      naturally.  Make sure the else still fires when the rewrite is
#      conservatively skipped.  Easier to keep the rewrite from firing
#      when else_branch is present; this asserts that behaviour.
def with_else():
    xs = [1, 2, 3]
    i = 0
    total = 0
    while i < len(xs):
        total += xs[i]
        i += 1
    else:
        total += 100
    return total
assert with_else() == 106
print("while-index-18", with_else())
