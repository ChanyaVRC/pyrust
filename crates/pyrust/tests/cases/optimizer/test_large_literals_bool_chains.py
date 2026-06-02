# Regression coverage for issue #2004 (large distinct-constant literals) and
# issue #2007 (long boolean or/and chains).  Both used to compile O(n^2); the
# optimizer (pass_dead_store_elim / pass_cse) and the line-number remap are now
# linear.  This fixture checks the *output* is still correct after linearization
# — the scaling itself is covered by Rust unit tests on the passes.

# --- Large list / tuple / dict literals with distinct constants --------------
N = 500
big_list = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9,
    10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
    20, 21, 22, 23, 24, 25, 26, 27, 28, 29,
]
print(len(big_list), sum(big_list), big_list[0], big_list[-1])

big_tuple = (
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9,
    10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
)
print(len(big_tuple), sum(big_tuple), big_tuple[5])

big_dict = {0: 0, 1: 10, 2: 20, 3: 30, 4: 40, 5: 50, 6: 60, 7: 70, 8: 80, 9: 90}
print(len(big_dict), big_dict[3], sum(big_dict.values()))

# Identical-element literal (the linear control case from #2004).
same = [7, 7, 7, 7, 7, 7, 7, 7]
print(len(same), sum(same), set(same))


# --- Long boolean or/and chains (#2007) -------------------------------------
def or_chain(a):
    # Short-circuit must stop at the first true comparison.
    return (
        a == 0
        or a == 1
        or a == 2
        or a == 3
        or a == 4
        or a == 5
        or a == 6
        or a == 7
    )


def and_chain(a):
    return (
        a != 0
        and a != 1
        and a != 2
        and a != 3
        and a != 4
        and a != 5
        and a != 6
        and a != 7
    )


for v in [-1, 0, 3, 7, 8]:
    print(v, or_chain(v), and_chain(v))


# Non-foldable operand: the comparison target is unknown at compile time, so the
# chain is compiled but not constant-folded (the #2007 control).
def or_chain_dyn(seq):
    n = len(seq)
    return n == 1 or n == 2 or n == 3 or n == 4 or n == 5


print(or_chain_dyn([]))
print(or_chain_dyn([10]))
print(or_chain_dyn([10, 20, 30]))


# Side-effecting short-circuit: confirms evaluation order is preserved after
# dead-store elimination touches the temporaries holding each term.
def trace_or(flags):
    seen = []

    def mark(i, val):
        seen.append(i)
        return val

    result = mark(0, flags[0]) or mark(1, flags[1]) or mark(2, flags[2])
    return result, seen


print(trace_or([False, False, True]))
print(trace_or([False, True, False]))
print(trace_or([True, False, False]))
