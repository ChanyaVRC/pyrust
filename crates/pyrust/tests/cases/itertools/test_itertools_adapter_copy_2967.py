# Parity fixture for issue #2967: itertools source-holding adapter
# copy/deepcopy semantics, tee copy/deepcopy, and pairwise/batched refusal.

import copy
import itertools


def show(label, fn):
    try:
        r = fn()
        print(f"{label}: OK -> {r!r}")
    except BaseException as e:
        print(f"{label}: ERR -> {type(e).__name__}: {e}")


# ---------------------------------------------------------------------------
# 1. Full fresh / mid / exhausted × copy / deepcopy × drain-order matrix
# ---------------------------------------------------------------------------
def run_matrix(name, make, step_mid=1):
    for mode, copier in [("copy", copy.copy), ("deepcopy", copy.deepcopy)]:
        for state in ["fresh", "mid", "exhausted"]:
            for order in ["orig-first", "copy-first"]:
                def case(copier=copier, state=state, order=order):
                    it = make()
                    pre = []
                    if state == "mid":
                        for _ in range(step_mid):
                            pre.append(next(it))
                    elif state == "exhausted":
                        pre = list(it)
                    c = copier(it)
                    if order == "orig-first":
                        a = list(it)
                        b = list(c)
                    else:
                        b = list(c)
                        a = list(it)
                    return (pre, a, b, type(c) is type(it), c is not it)

                show(f"{name}/{mode}/{state}/{order}", case)


run_matrix("chain", lambda: itertools.chain([1, 2], [3, 4]))
run_matrix("chain.from_iterable", lambda: itertools.chain.from_iterable([[1, 2], [3, 4]]))
run_matrix("zip_longest", lambda: itertools.zip_longest([1, 2, 3], [10, 20], fillvalue=-1))
run_matrix("accumulate", lambda: itertools.accumulate([1, 2, 3, 4]))
run_matrix("accumulate_init", lambda: itertools.accumulate([1, 2, 3], initial=100))
run_matrix("compress", lambda: itertools.compress(["a", "b", "c", "d"], [1, 0, 1, 1]))
run_matrix("starmap", lambda: itertools.starmap(lambda a, b: a + b, [(1, 2), (3, 4), (5, 6)]))
run_matrix("filterfalse", lambda: itertools.filterfalse(lambda x: x % 2 == 0, [1, 2, 3, 4, 5]))
run_matrix("filterfalse_none", lambda: itertools.filterfalse(None, [0, 1, "", "x", False, 2]))
run_matrix("dropwhile", lambda: itertools.dropwhile(lambda x: x < 3, [1, 2, 3, 1, 4]))
run_matrix("takewhile", lambda: itertools.takewhile(lambda x: x < 3, [1, 2, 3, 1]))
run_matrix("islice", lambda: itertools.islice([10, 20, 30, 40, 50, 60], 1, 6, 2))


# ---------------------------------------------------------------------------
# 2. chain & chain.from_iterable interleaved shallow & exhausted-gen deepcopy
# ---------------------------------------------------------------------------
def t_chain_shallow_interleaved():
    c = itertools.chain([1, 2], [3, 4])
    c2 = copy.copy(c)
    a = next(c)
    b = next(c2)
    return (a, b, list(c), list(c2))


def t_chain_mid_shallow_interleaved():
    c = itertools.chain([1, 2], [3, 4], [5, 6])
    p = next(c)
    c2 = copy.copy(c)
    a = next(c)
    b = next(c2)
    return (p, a, b, list(c), list(c2))


def t_chain_gen_shallow():
    def g():
        yield 1
        yield 2

    c = itertools.chain(g(), [3, 4])
    c_sh = copy.copy(c)
    r_fresh = (next(c), next(c_sh), list(c), list(c_sh))
    c_mid = itertools.chain(g(), [3, 4])
    next(c_mid)
    c_mid_sh = copy.copy(c_mid)
    r_mid = (list(c_mid), list(c_mid_sh))
    return (r_fresh, r_mid)


def t_chain_from_iterable_gen_inner_deepcopy():
    def g():
        yield 1
        yield 2

    c = itertools.chain.from_iterable([g(), [3, 4]])
    next(c)
    return copy.deepcopy(c)


def t_chain_gen_exhausted_deepcopy():
    def g():
        yield 1

    c = itertools.chain(g())
    list(c)
    return list(copy.deepcopy(c))


def t_chain_fi_gen_exhausted_deepcopy():
    def g():
        yield [1]
        yield [2]

    c = itertools.chain.from_iterable(g())
    list(c)
    return list(copy.deepcopy(c))


def t_chain_fi_cycle_deepcopy():
    box = [[1, 2]]
    c = itertools.chain.from_iterable(box)
    box.append(c)
    c2 = copy.deepcopy(c)
    return (next(c2), next(c2), type(c2) is type(c))


show("chain shallow interleaved", t_chain_shallow_interleaved)
show("chain mid shallow interleaved", t_chain_mid_shallow_interleaved)
show("chain(gen) shallow", t_chain_gen_shallow)
show("chain.from_iterable([gen]) mid deepcopy", t_chain_from_iterable_gen_inner_deepcopy)
show("chain(gen) exhausted deepcopy", t_chain_gen_exhausted_deepcopy)
show("chain.from_iterable(gen) exhausted deepcopy", t_chain_fi_gen_exhausted_deepcopy)
show("chain.from_iterable cycle deepcopy", t_chain_fi_cycle_deepcopy)


# ---------------------------------------------------------------------------
# 3. groupby & _grouper shallow/deep semantics
# ---------------------------------------------------------------------------
for mode, copier in [("copy", copy.copy), ("deepcopy", copy.deepcopy)]:
    for state in ["fresh", "mid_before_g", "mid_after_g", "exhausted"]:
        for order in ["orig-first", "copy-first"]:
            def case_gb(copier=copier, state=state, order=order):
                it = itertools.groupby([1, 1, 2, 2, 3])
                pre = []
                if state == "mid_before_g":
                    k, g = next(it)
                    pre.append(k)
                elif state == "mid_after_g":
                    k, g = next(it)
                    pre.append((k, next(g)))
                elif state == "exhausted":
                    pre = [(k, list(g)) for k, g in it]
                c = copier(it)
                if order == "orig-first":
                    a = [(k, list(g)) for k, g in it]
                    b = [(k, list(g)) for k, g in c]
                else:
                    b = [(k, list(g)) for k, g in c]
                    a = [(k, list(g)) for k, g in it]
                return (pre, a, b, type(c) is type(it))

            show(f"groupby/{mode}/{state}/{order}", case_gb)


def t_groupby_keyfunc_deepcopy():
    it = itertools.groupby(["aa", "ab", "ba", "bb"], key=lambda s: s[0])
    k, g = next(it)
    first = next(g)
    c = copy.deepcopy(it)
    return (k, first, [(k2, list(g2)) for k2, g2 in c], [(k2, list(g2)) for k2, g2 in it])


def t_grouper_shallow():
    it = itertools.groupby([1, 1, 1, 2])
    _, g = next(it)
    first = next(g)
    g2 = copy.copy(g)
    return (first, list(g), list(g2))


def t_grouper_deep():
    it = itertools.groupby([1, 1, 1, 2])
    _, g = next(it)
    first = next(g)
    g2 = copy.deepcopy(g)
    return (first, list(g2), list(g))


def t_grouper_paired_deepcopy():
    gb = itertools.groupby([1, 1, 1, 2, 2])
    _, g = next(gb)
    gb2, g2 = copy.deepcopy((gb, g))
    return (
        list(g2),
        [(k, list(gr)) for k, gr in gb2],
        list(g),
        [(k, list(gr)) for k, gr in gb],
    )


def t_grouper_stale_copy():
    gb = itertools.groupby([1, 1, 2])
    _, g_old = next(gb)
    next(gb)
    return (list(copy.copy(g_old)), list(copy.deepcopy(g_old)))


def t_groupby_gen_exhausted_deepcopy():
    def gen():
        yield 1
        yield 2

    gb = itertools.groupby(gen())
    list(gb)
    return copy.deepcopy(gb)


show("groupby keyfunc deepcopy", t_groupby_keyfunc_deepcopy)
show("_grouper shallow", t_grouper_shallow)
show("_grouper deep", t_grouper_deep)
show("_grouper paired deepcopy", t_grouper_paired_deepcopy)
show("_grouper stale copy/deepcopy", t_grouper_stale_copy)
show("groupby(gen) exhausted deepcopy", t_groupby_gen_exhausted_deepcopy)


# ---------------------------------------------------------------------------
# 4. tee (itertools._tee) copy / deepcopy / __copy__ / re-tee
# ---------------------------------------------------------------------------
for mode, copier in [("copy", copy.copy), ("deepcopy", copy.deepcopy)]:
    for state in ["fresh", "mid", "exhausted"]:
        for order in ["orig-first", "copy-first"]:
            def case_tee(copier=copier, state=state, order=order):
                t1, t2 = itertools.tee([[1], [2], [3]])
                pre = []
                if state == "mid":
                    pre.append(next(t1))
                elif state == "exhausted":
                    pre = list(t1)
                c = copier(t1)
                if order == "orig-first":
                    a = list(t1)
                    b = list(c)
                else:
                    b = list(c)
                    a = list(t1)
                same = (a[0] is b[0]) if (a and b) else None
                return (pre, a, b, same, type(c) is type(t1), c is not t1)

            show(f"tee/{mode}/{state}/{order}", case_tee)


def t_tee_dunder_copy():
    t1, _ = itertools.tee([[1], [2], [3]])
    p = next(t1)
    c = t1.__copy__()
    a = list(t1)
    b = list(c)
    return (hasattr(t1, "__copy__"), p, a, b, a[0] is b[0])


def t_tee_gen_shallow():
    def g():
        yield 1
        yield 2
        yield 3

    t1, t2 = itertools.tee(g())
    p = next(t1)
    t1_sh = copy.copy(t1)
    return (p, list(t1), list(t1_sh), list(t2))


def t_tee_gen_exhausted_deepcopy():
    def g():
        yield 1
        yield 2

    t1, t2 = itertools.tee(g())
    list(t1)
    return copy.deepcopy(t1)


def t_tee_paired_deepcopy():
    t1, t2 = itertools.tee([[1], [2], [3]])
    next(t1)
    c1, c2 = copy.deepcopy((t1, t2))
    r1 = list(c1)
    r2 = list(c2)
    return (r1, r2, r1[0] is r2[1], r1[0] is next(t1))


def t_tee_retee():
    t1, t2 = itertools.tee([1, 2, 3])
    next(t1)
    a, b = itertools.tee(t1)
    return (a is t1, list(t1), list(a), list(b), list(t2))


show("tee.__copy__", t_tee_dunder_copy)
show("tee(gen) shallow", t_tee_gen_shallow)
show("tee(gen) exhausted deepcopy", t_tee_gen_exhausted_deepcopy)
show("tee paired deepcopy", t_tee_paired_deepcopy)
show("tee(t1) re-tee", t_tee_retee)


# ---------------------------------------------------------------------------
# 5. cycle, count, repeat, islice, accumulate, zip_longest, dropwhile, takewhile
# ---------------------------------------------------------------------------
def t_cycle_mid_1st_pass_shallow():
    it = itertools.cycle([1, 2, 3])
    p = next(it)
    c = copy.copy(it)
    a = [next(it) for _ in range(5)]
    b = [next(c) for _ in range(5)]
    return (p, a, b)


def t_cycle_post_1st_pass():
    it = itertools.cycle([[1], [2]])
    pre = [next(it) for _ in range(3)]
    c_sh = copy.copy(it)
    c_dp = copy.deepcopy(it)
    a = [next(it) for _ in range(4)]
    b = [next(c_sh) for _ in range(4)]
    d = [next(c_dp) for _ in range(4)]
    return (pre, a, b, d, a[0] is b[0], a[0] is d[0])


def t_cycle_gen_post_1st_pass_deepcopy():
    def g():
        yield 1
        yield 2

    it = itertools.cycle(g())
    pre = [next(it) for _ in range(3)]
    c = copy.deepcopy(it)
    return (pre, [next(it) for _ in range(3)], [next(c) for _ in range(3)])


def t_cycle_gen_mid_1st_pass_deepcopy():
    def g():
        yield 1
        yield 2

    it = itertools.cycle(g())
    next(it)
    return copy.deepcopy(it)


def t_count_repeat():
    cnt = itertools.count(10, 2)
    a = next(cnt)
    cnt_sh = copy.copy(cnt)
    cnt_dp = copy.deepcopy(cnt)
    rep = itertools.repeat([1], 3)
    r0 = next(rep)
    rep_sh = copy.copy(rep)
    rep_dp = copy.deepcopy(rep)
    sh_items = list(rep_sh)
    dp_items = list(rep_dp)
    return (
        (a, next(cnt_sh), next(cnt_dp), next(cnt)),
        (r0, sh_items, dp_items, sh_items[0] is r0, dp_items[0] is r0),
    )


def t_islice_interleaved_and_gen_exhausted():
    it = itertools.islice(range(10), 1, 8, 2)
    p = next(it)
    c = copy.copy(it)
    a = next(it)
    b = next(c)
    r1 = (p, a, b, list(it), list(c))

    def g():
        yield 1
        yield 2
        yield 3

    it2 = itertools.islice(g(), 1)
    list(it2)
    r2 = list(copy.deepcopy(it2))
    return (r1, r2)


def t_accumulate_mutable():
    it = itertools.accumulate([[1], [2], [3]])
    p = next(it)
    c_sh = copy.copy(it)
    c_dp = copy.deepcopy(it)
    p.append(99)
    init = [10]
    it2 = itertools.accumulate([[1], [2]], initial=init)
    i_sh = next(copy.copy(it2))
    i_dp = next(copy.deepcopy(it2))
    return (next(c_sh), next(c_dp), i_sh is init, i_dp is init, i_sh, i_dp)


def t_zip_longest_mid_and_gen_exhausted():
    it = itertools.zip_longest([1, 2, 3], [10], fillvalue=[0])
    p1 = next(it)
    p2 = next(it)
    c_sh = copy.copy(it)
    c_dp = copy.deepcopy(it)
    a = list(c_dp)
    b = list(c_sh)
    rest = list(it)

    def g():
        yield 10

    it2 = itertools.zip_longest([1, 2, 3], g(), fillvalue=-1)
    next(it2)
    next(it2)
    c2 = copy.deepcopy(it2)
    return (p1, p2, a, b, rest, b[0][1] is p2[1], a[0][1] is p2[1], list(it2), list(c2))


def t_dropwhile_pred_not_recalled():
    calls = []

    def pred(x):
        calls.append(x)
        return x < 3

    it = itertools.dropwhile(pred, [1, 2, 3, 1, 2])
    first = next(it)
    c_sh = copy.copy(it)
    c_dp = copy.deepcopy(it)
    return (first, list(c_dp), list(c_sh), calls)


def t_takewhile_gen_exhausted_deepcopy():
    def g():
        yield 1
        yield 5
        yield 2

    it = itertools.takewhile(lambda x: x < 3, g())
    list(it)
    return copy.deepcopy(it)


show("cycle mid 1st-pass shallow", t_cycle_mid_1st_pass_shallow)
show("cycle post 1st-pass shallow & deep", t_cycle_post_1st_pass)
show("cycle(gen) post 1st-pass deepcopy", t_cycle_gen_post_1st_pass_deepcopy)
show("cycle(gen) mid 1st-pass deepcopy", t_cycle_gen_mid_1st_pass_deepcopy)
show("count & repeat", t_count_repeat)
show("islice interleaved & gen exhausted", t_islice_interleaved_and_gen_exhausted)
show("accumulate mutable total & initial", t_accumulate_mutable)
show("zip_longest mid & gen exhausted", t_zip_longest_mid_and_gen_exhausted)
show("dropwhile pred not recalled", t_dropwhile_pred_not_recalled)
show("takewhile(gen) exhausted deepcopy", t_takewhile_gen_exhausted_deepcopy)


# ---------------------------------------------------------------------------
# 6. Live unpicklable generator source across all adapters
# ---------------------------------------------------------------------------
def gen3():
    yield 1
    yield 2
    yield 3


for name, make in [
    ("chain(gen)", lambda: itertools.chain(gen3())),
    ("chain.from_iterable(gen)", lambda: itertools.chain.from_iterable([1] for _ in range(2))),
    ("islice(gen)", lambda: itertools.islice(gen3(), 2)),
    ("takewhile(gen)", lambda: itertools.takewhile(lambda x: x < 3, gen3())),
    ("dropwhile(gen)", lambda: itertools.dropwhile(lambda x: x < 2, gen3())),
    ("filterfalse(gen)", lambda: itertools.filterfalse(lambda x: x == 2, gen3())),
    ("starmap(gen)", lambda: itertools.starmap(lambda a: a, ((x,) for x in [1, 2]))),
    ("compress(gen)", lambda: itertools.compress(gen3(), [1, 1, 1])),
    ("accumulate(gen)", lambda: itertools.accumulate(gen3())),
    ("zip_longest(gen)", lambda: itertools.zip_longest(gen3(), [10, 20])),
    ("groupby(gen)", lambda: itertools.groupby(gen3())),
    ("tee(gen)", lambda: itertools.tee(gen3())[0]),
    ("cycle(gen)", lambda: itertools.cycle(gen3())),
]:
    for mode, copier in [("copy", copy.copy), ("deepcopy", copy.deepcopy)]:
        def case_gen(make=make, copier=copier):
            it = make()
            c = copier(it)
            return type(c) is type(it)

        show(f"{name}/{mode}", case_gen)


# ---------------------------------------------------------------------------
# 7. pairwise & batched refusal (fresh, mid, exhausted, subclasses, overrides)
# ---------------------------------------------------------------------------
for name, make in [
    ("pairwise", lambda: itertools.pairwise([1, 2, 3, 4])),
    ("batched", lambda: itertools.batched([1, 2, 3, 4], 2)),
]:
    it0 = make()
    print(f"{name} hasattr __copy__/__deepcopy__:", hasattr(it0, "__copy__"), hasattr(it0, "__deepcopy__"))
    for mode, copier in [("copy", copy.copy), ("deepcopy", copy.deepcopy)]:
        for state in ["fresh", "mid", "exhausted"]:
            def case_refuse(make=make, copier=copier, state=state):
                it = make()
                if state == "mid":
                    next(it)
                elif state == "exhausted":
                    list(it)
                return copier(it)

            show(f"{name}/{mode}/{state}", case_refuse)


class MyPairwise(itertools.pairwise):
    pass


class MyBatched(itertools.batched):
    pass


class MyPairwiseCustomCopy(itertools.pairwise):
    def __copy__(self):
        return "custom-pairwise-copy"

    def __deepcopy__(self, memo):
        return "custom-pairwise-deepcopy"


show("MyPairwise/copy", lambda: copy.copy(MyPairwise([1, 2])))
show("MyPairwise/deepcopy", lambda: copy.deepcopy(MyPairwise([1, 2])))
show("MyBatched/copy", lambda: copy.copy(MyBatched([1, 2], 1)))
show("MyBatched/deepcopy", lambda: copy.deepcopy(MyBatched([1, 2], 1)))
show("MyPairwiseCustomCopy/copy", lambda: copy.copy(MyPairwiseCustomCopy([1, 2])))
show("MyPairwiseCustomCopy/deepcopy", lambda: copy.deepcopy(MyPairwiseCustomCopy([1, 2])))
