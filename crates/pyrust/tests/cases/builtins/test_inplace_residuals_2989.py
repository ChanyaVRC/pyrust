# Parity fixture for issue #2989: in-place operator residuals across list,
# list subclasses, collections.deque, deque subclasses, bytearray, and
# bytearray subclasses.

from collections import deque


class LSub(list):
    pass


class DSub(deque):
    pass


class BASub(bytearray):
    pass


class BSubRAdd(bytes):
    def __radd__(self, other):
        return ("BYTES_RADD", type(other).__name__)


class HasRAdd:
    def __radd__(self, other):
        return ("RADD", type(other).__name__)


class HasRMul:
    def __rmul__(self, other):
        return ("RMUL", type(other).__name__)


class IterWithRAdd:
    def __iter__(self):
        yield 99

    def __radd__(self, other):
        return ("RADD", type(other).__name__)


class IterWithRAddNI:
    def __iter__(self):
        yield 99

    def __radd__(self, other):
        return NotImplemented


class Idx:
    def __init__(self, v):
        self.v = v

    def __index__(self):
        return self.v


class IdxHuge:
    def __index__(self):
        return 1 << 100


class IdxBoom:
    def __index__(self):
        raise ValueError("idx_boom")


class BothIndexAndRMul:
    def __index__(self):
        return 3

    def __rmul__(self, other):
        return ("RMUL", type(other).__name__)


class BothIndexAndRMulNI:
    def __index__(self):
        return 3

    def __rmul__(self, other):
        return NotImplemented


class IntSub(int):
    pass


def check(label, fn):
    try:
        r = fn()
        print(f"{label}: OK -> {r!r}")
    except BaseException as e:
        print(f"{label}: ERR -> {type(e).__name__}: {e}")


# ---------------------------------------------------------------------------
# Items 1 & 2: incremental list += / LSub += / list.extend / LSub.extend(self)
# ---------------------------------------------------------------------------
def raising_gen():
    yield 10
    yield 20
    raise RuntimeError("gen_boom")


def t_list_iadd_partial():
    a = [1]
    b = a
    try:
        a += raising_gen()
    except RuntimeError as e:
        return (a, a is b, str(e))


def t_lsub_iadd_partial():
    a = LSub([1])
    b = a
    try:
        a += raising_gen()
    except RuntimeError as e:
        return (list(a), type(a).__name__, a is b, str(e))


def t_list_iadd_interleave():
    a = [1]

    def mutating():
        yield 10
        a.append(99)
        yield 20

    a += mutating()
    return a


def t_lsub_iadd_interleave():
    a = LSub([1])

    def mutating():
        yield 10
        a.append(99)
        yield 20

    a += mutating()
    return (list(a), type(a).__name__)


def t_lsub_extend_self():
    a = LSub([1, 2])
    b = a
    a.extend(a)
    return (list(a), type(a).__name__, a is b)


def t_lsub_iadd_self():
    a = LSub([1, 2])
    b = a
    a += a
    return (list(a), type(a).__name__, a is b)


class LSubCustomIter(list):
    def __iter__(self):
        yield 777


class LSubCapIter(list):
    def __iter__(self):
        n = 0
        for x in super().__iter__():
            yield x
            n += 1
            if n >= 5:
                break


class LSubBoomIter(list):
    def __iter__(self):
        yield 10
        raise RuntimeError("iter_boom")


def t_lsub_custom_iter_self_extend():
    a = LSubCustomIter([1, 2])
    a.extend(a)
    return (repr(a), len(a))


def t_lsub_custom_iter_self_iadd():
    a = LSubCustomIter([1, 2])
    a += a
    return (repr(a), len(a))


def t_lsub_cap_iter_self_extend():
    a = LSubCapIter([1, 2])
    a.extend(a)
    return repr(a)


def t_lsub_boom_iter_self():
    a = LSubBoomIter([1, 2])
    try:
        a.extend(a)
    except RuntimeError:
        return repr(a)


def t_lsub_boom_iter_other():
    a = LSubBoomIter([1, 2])
    b = LSubBoomIter([3, 4])
    try:
        a.extend(b)
    except RuntimeError:
        return repr(a)


check("1.1 list += raising_gen", t_list_iadd_partial)
check("1.2 LSub += raising_gen", t_lsub_iadd_partial)
check("1.3 list += interleave", t_list_iadd_interleave)
check("1.4 LSub += interleave", t_lsub_iadd_interleave)
check("2.1 LSub.extend(self)", t_lsub_extend_self)
check("2.2 LSub += self", t_lsub_iadd_self)
check("2.3 LSubCustomIter.extend(self)", t_lsub_custom_iter_self_extend)
check("2.4 LSubCustomIter += self", t_lsub_custom_iter_self_iadd)
check("2.5 LSubCapIter.extend(self)", t_lsub_cap_iter_self_extend)
check("2.6 LSubBoomIter.extend(self)", t_lsub_boom_iter_self)
check("2.7 LSubBoomIter.extend(other)", t_lsub_boom_iter_other)


# ---------------------------------------------------------------------------
# Item 3: plain list *= Idx() identity, overflow, MemoryError, __rmul__
# ---------------------------------------------------------------------------
def t_list_imul_idx():
    a = [1, 2]
    b = a
    a *= Idx(3)
    return (a, a is b)


def t_list_imul_idx_zero():
    a = [1, 2]
    b = a
    a *= Idx(0)
    return (a, a is b)


def t_list_imul_idx_neg():
    a = [1, 2]
    b = a
    a *= Idx(-2)
    return (a, a is b)


def t_list_imul_intsub():
    a = [1, 2]
    b = a
    a *= IntSub(2)
    return (a, a is b)


def t_list_imul_boom():
    a = [1, 2]
    try:
        a *= IdxBoom()
    except ValueError as e:
        return (a, str(e))


def t_list_imul_huge():
    a = [1]
    a *= IdxHuge()
    return a


def t_list_imul_bigint():
    a = [1]
    a *= 1 << 100
    return a


def t_list_imul_memerr():
    a = [1]
    try:
        a *= 1 << 62
    except MemoryError:
        return ("MemoryError", a)


def t_list_imul_rmul():
    a = [1]
    a *= HasRMul()
    return a


def t_list_imul_both():
    a = [1]
    a *= BothIndexAndRMul()
    return a


def t_list_imul_both_ni():
    a = [1]
    b = a
    a *= BothIndexAndRMulNI()
    return (a, a is b)


def t_list_imul_lsub():
    a = [1]
    a *= LSub([2])
    return a


check("3.1 [1,2] *= Idx(3)", t_list_imul_idx)
check("3.2 [1,2] *= Idx(0)", t_list_imul_idx_zero)
check("3.3 [1,2] *= Idx(-2)", t_list_imul_idx_neg)
check("3.4 [1,2] *= IntSub(2)", t_list_imul_intsub)
check("3.5 [1,2] *= IdxBoom()", t_list_imul_boom)
check("3.6 [1] *= IdxHuge()", t_list_imul_huge)
check("3.7 [1] *= (1<<100)", t_list_imul_bigint)
check("3.8 [1] *= (1<<62)", t_list_imul_memerr)
check("3.9 [1] *= HasRMul()", t_list_imul_rmul)
check("3.10 [1] *= BothIndexAndRMul()", t_list_imul_both)
check("3.11 [1] *= BothIndexAndRMulNI()", t_list_imul_both_ni)
check("3.12 [1] *= LSub([2])", t_list_imul_lsub)
check("3.13 [1].__imul__(IdxHuge())", lambda: [1].__imul__(IdxHuge()))
check("3.14 [1].__imul__(HasRMul())", lambda: [1].__imul__(HasRMul()))


# ---------------------------------------------------------------------------
# Item 4: [1] += HasRAdd() vs LSub([1]) += HasRAdd()
# ---------------------------------------------------------------------------
class LSubNI(list):
    def __iadd__(self, other):
        return NotImplemented


def t_list_iadd_hasradd():
    a = [1]
    a += HasRAdd()
    return a


def t_lsub_iadd_hasradd():
    a = LSub([1])
    a += HasRAdd()
    return a


def t_list_iadd_iter_radd():
    a = [1]
    a += IterWithRAdd()
    return a


def t_list_iadd_iter_radd_ni():
    a = [1]
    b = a
    a += IterWithRAddNI()
    return (a, a is b)


def t_lsub_iadd_iter_radd():
    a = LSub([1])
    b = a
    a += IterWithRAdd()
    return (list(a), type(a).__name__, a is b)


def t_lsub_ni_iadd_hasradd():
    a = LSubNI([1])
    a += HasRAdd()
    return a


check("4.1 [1] += HasRAdd()", t_list_iadd_hasradd)
check("4.2 LSub([1]) += HasRAdd()", t_lsub_iadd_hasradd)
check("4.3 [1] += IterWithRAdd()", t_list_iadd_iter_radd)
check("4.4 [1] += IterWithRAddNI()", t_list_iadd_iter_radd_ni)
check("4.5 LSub([1]) += IterWithRAdd()", t_lsub_iadd_iter_radd)
check("4.6 [1].__iadd__(HasRAdd())", lambda: [1].__iadd__(HasRAdd()))
check("4.7 [1].__add__(HasRAdd())", lambda: [1].__add__(HasRAdd()))
check("4.8 LSubNI([1]) += HasRAdd()", t_lsub_ni_iadd_hasradd)


# ---------------------------------------------------------------------------
# Item 5: LSub * <non-int> / HasRMul / Idx / IdxHuge
# ---------------------------------------------------------------------------
def t_lsub_imul_hasrmul():
    a = LSub([1])
    a *= HasRMul()
    return a


def t_lsub_imul_both():
    a = LSub([1])
    a *= BothIndexAndRMul()
    return a


def t_lsub_imul_both_ni():
    a = LSub([1])
    b = a
    a *= BothIndexAndRMulNI()
    return (list(a), type(a).__name__, a is b)


def t_lsub_imul_str():
    a = LSub([1])
    a *= "x"
    return a


def t_lsub_imul_idxhuge():
    a = LSub([1])
    a *= IdxHuge()
    return a


check("5.1 LSub([1]) * 'x'", lambda: LSub([1]) * "x")
check("5.2 'x' * LSub([1])", lambda: "x" * LSub([1]))
check("5.3 LSub([1]) * HasRMul()", lambda: LSub([1]) * HasRMul())
check("5.4 HasRMul() * LSub([1])", lambda: HasRMul() * LSub([1]))
check("5.5 LSub([1]) * (1<<100)", lambda: LSub([1]) * (1 << 100))
check("5.6 LSub([1]) * IdxHuge()", lambda: LSub([1]) * IdxHuge())
check("5.7 IdxHuge() * LSub([1])", lambda: IdxHuge() * LSub([1]))
check("5.8 [1] * IdxHuge()", lambda: [1] * IdxHuge())
check("5.9 LSub([1]) * Idx(3)", lambda: (LSub([1]) * Idx(3), type(LSub([1]) * Idx(3)).__name__))
check("5.10 Idx(3) * LSub([1])", lambda: (Idx(3) * LSub([1]), type(Idx(3) * LSub([1])).__name__))
check("5.11 LSub([1]) + HasRAdd()", lambda: LSub([1]) + HasRAdd())
check("5.12 LSub([1]) *= HasRMul()", t_lsub_imul_hasrmul)
check("5.13 LSub([1]) *= BothIndexAndRMul()", t_lsub_imul_both)
check("5.14 LSub([1]) *= BothIndexAndRMulNI()", t_lsub_imul_both_ni)
check("5.15 LSub([1]) *= 'x'", t_lsub_imul_str)
check("5.16 LSub([1]) *= IdxHuge()", t_lsub_imul_idxhuge)
check("5.17 LSub([1]).__mul__('x')", lambda: LSub([1]).__mul__("x"))
check("5.18 LSub([1]).__mul__(IdxHuge())", lambda: LSub([1]).__mul__(IdxHuge()))


# ---------------------------------------------------------------------------
# Item 6: collections.deque & DSub += / *= / extend / extendleft / maxlen
# ---------------------------------------------------------------------------
class DSubNI(deque):
    def __iadd__(self, other):
        return NotImplemented

    def __imul__(self, n):
        return NotImplemented


class DSubCustomIter(deque):
    def __iter__(self):
        yield 99


class DSubBoomIter(deque):
    def __iter__(self):
        yield 10
        raise RuntimeError("dsub_boom")


def t_deque_iadd():
    d = deque([1, 2])
    alias = d
    d += [3, 4]
    return (d, d is alias)


def t_dsub_iadd():
    d = DSub([1, 2])
    alias = d
    d += [3, 4]
    return (d, type(d).__name__, d is alias)


def t_deque_iadd_self():
    d = deque([1, 2])
    alias = d
    d += d
    return (d, d is alias)


def t_dsub_iadd_self():
    d = DSub([1, 2])
    alias = d
    d += d
    return (d, type(d).__name__, d is alias)


def t_dsub_extend_self():
    d = DSub([1, 2])
    alias = d
    d.extend(d)
    return (d, type(d).__name__, d is alias)


def t_dsub_extendleft_self():
    d = DSub([1, 2])
    alias = d
    d.extendleft(d)
    return (d, type(d).__name__, d is alias)


def t_deque_iadd_partial():
    d = deque([1])
    alias = d
    try:
        d += raising_gen()
    except RuntimeError as e:
        return (d, d is alias, str(e))


def t_dsub_iadd_partial():
    d = DSub([1])
    alias = d
    try:
        d += raising_gen()
    except RuntimeError as e:
        return (d, type(d).__name__, d is alias, str(e))


def t_deque_imul_int():
    d = deque([1, 2])
    alias = d
    d *= 3
    return (d, d is alias)


def t_deque_imul_idx():
    d = deque([1, 2])
    alias = d
    d *= Idx(3)
    return (d, d is alias)


def t_dsub_imul_idx():
    d = DSub([1, 2])
    alias = d
    d *= Idx(3)
    return (d, type(d).__name__, d is alias)


def t_dsub_imul_zero():
    d = DSub([1, 2])
    alias = d
    d *= Idx(0)
    return (d, type(d).__name__, d is alias)


def t_dsub_imul_neg():
    d = DSub([1, 2])
    alias = d
    d *= Idx(-1)
    return (d, type(d).__name__, d is alias)


def t_deque_maxlen_iadd():
    d = deque([1, 2], maxlen=4)
    alias = d
    d += [3, 4, 5]
    return (d, d.maxlen, d is alias)


def t_dsub_maxlen_imul():
    d = DSub([1, 2, 3], maxlen=4)
    alias = d
    d *= 3
    return (d, d.maxlen, type(d).__name__, d is alias)


def t_deque_iadd_hasradd():
    d = deque([1])
    d += HasRAdd()
    return d


def t_dsub_iadd_hasradd():
    d = DSub([1])
    d += HasRAdd()
    return d


def t_deque_iadd_iter_radd():
    d = deque([1])
    d += IterWithRAdd()
    return d


def t_dsub_iadd_iter_radd():
    d = DSub([1])
    alias = d
    d += IterWithRAdd()
    return (d, type(d).__name__, d is alias)


def t_deque_imul_hasrmul():
    d = deque([1])
    d *= HasRMul()
    return d


def t_dsub_imul_hasrmul():
    d = DSub([1])
    d *= HasRMul()
    return d


def t_deque_imul_both():
    d = deque([1])
    d *= BothIndexAndRMul()
    return d


def t_dsub_imul_both_ni():
    d = DSub([1])
    alias = d
    d *= BothIndexAndRMulNI()
    return (d, type(d).__name__, d is alias)


def t_deque_imul_str():
    d = deque([1])
    d *= "x"
    return d


def t_dsub_imul_huge():
    d = DSub([1])
    d *= IdxHuge()
    return d


def t_dsub_ni_iadd_deque():
    d = DSubNI([1])
    alias = d
    d += DSubNI([2])
    return (d, type(d).__name__, d is alias)


def t_dsub_ni_iadd_hasradd():
    d = DSubNI([1])
    d += HasRAdd()
    return d


def t_dsub_ni_imul_int():
    d = DSubNI([1])
    alias = d
    d *= 3
    return (d, type(d).__name__, d is alias)


def t_dsub_ni_imul_hasrmul():
    d = DSubNI([1])
    d *= HasRMul()
    return d


def t_dsub_custom_iter_self():
    d = DSubCustomIter([1, 2])
    d += d
    return list(deque.__iter__(d))


def t_dsub_boom_iter_self():
    d = DSubBoomIter([1, 2])
    try:
        d.extend(d)
    except RuntimeError:
        return list(deque.__iter__(d))


def t_dsub_boom_iter_other():
    d = DSubBoomIter([1, 2])
    o = DSubBoomIter([3, 4])
    try:
        d.extend(o)
    except RuntimeError:
        return list(deque.__iter__(d))


check("6.1 deque += [3,4]", t_deque_iadd)
check("6.2 DSub += [3,4]", t_dsub_iadd)
check("6.3 deque += self", t_deque_iadd_self)
check("6.4 DSub += self", t_dsub_iadd_self)
check("6.5 DSub.extend(self)", t_dsub_extend_self)
check("6.6 DSub.extendleft(self)", t_dsub_extendleft_self)
check("6.7 deque += raising_gen", t_deque_iadd_partial)
check("6.8 DSub += raising_gen", t_dsub_iadd_partial)
check("6.9 deque *= 3", t_deque_imul_int)
check("6.10 deque *= Idx(3)", t_deque_imul_idx)
check("6.11 DSub *= Idx(3)", t_dsub_imul_idx)
check("6.12 DSub *= Idx(0)", t_dsub_imul_zero)
check("6.13 DSub *= Idx(-1)", t_dsub_imul_neg)
check("6.14 deque(maxlen=4) += [3,4,5]", t_deque_maxlen_iadd)
check("6.15 DSub(maxlen=4) *= 3", t_dsub_maxlen_imul)
check("6.16 deque += HasRAdd()", t_deque_iadd_hasradd)
check("6.17 DSub += HasRAdd()", t_dsub_iadd_hasradd)
check("6.18 deque += IterWithRAdd()", t_deque_iadd_iter_radd)
check("6.19 DSub += IterWithRAdd()", t_dsub_iadd_iter_radd)
check("6.20 deque *= HasRMul()", t_deque_imul_hasrmul)
check("6.21 DSub *= HasRMul()", t_dsub_imul_hasrmul)
check("6.22 deque *= BothIndexAndRMul()", t_deque_imul_both)
check("6.23 DSub *= BothIndexAndRMulNI()", t_dsub_imul_both_ni)
check("6.24 deque *= 'x'", t_deque_imul_str)
check("6.25 DSub *= IdxHuge()", t_dsub_imul_huge)
check("6.26 deque + HasRAdd()", lambda: deque([1]) + HasRAdd())
check("6.27 DSub + HasRAdd()", lambda: DSub([1]) + HasRAdd())
check("6.28 deque * HasRMul()", lambda: deque([1]) * HasRMul())
check("6.29 DSub * HasRMul()", lambda: DSub([1]) * HasRMul())
check("6.30 'x' * deque([1])", lambda: "x" * deque([1]))
check("6.31 'x' * DSub([1])", lambda: "x" * DSub([1]))
check("6.32 deque * IdxHuge()", lambda: deque([1]) * IdxHuge())
check("6.33 IdxHuge() * deque", lambda: IdxHuge() * deque([1]))
check("6.34 DSubNI += DSubNI", t_dsub_ni_iadd_deque)
check("6.35 DSubNI += HasRAdd", t_dsub_ni_iadd_hasradd)
check("6.36 DSubNI *= 3", t_dsub_ni_imul_int)
check("6.37 DSubNI *= HasRMul", t_dsub_ni_imul_hasrmul)
check("6.38 DSubCustomIter += self", t_dsub_custom_iter_self)
check("6.39 DSubBoomIter.extend(self)", t_dsub_boom_iter_self)
check("6.40 DSubBoomIter.extend(other)", t_dsub_boom_iter_other)
check("6.41 deque.__iadd__(HasRAdd)", lambda: deque([1]).__iadd__(HasRAdd()))
check("6.42 deque.__imul__(Idx(3))", lambda: deque([1]).__imul__(Idx(3)))
check("6.43 deque.__imul__(IdxHuge())", lambda: deque([1]).__imul__(IdxHuge()))
check("6.44 deque.__imul__(HasRMul())", lambda: deque([1]).__imul__(HasRMul()))
check("6.45 deque.__mul__(HasRMul())", lambda: deque([1]).__mul__(HasRMul()))


# ---------------------------------------------------------------------------
# Item 7: bytearray & BASub += / *= / + / *
# ---------------------------------------------------------------------------
def t_ba_imul_idx():
    a = bytearray(b"ab")
    b = a
    a *= Idx(3)
    return (a, a is b)


def t_basub_imul_idx():
    a = BASub(b"ab")
    b = a
    a *= Idx(3)
    return (a, type(a).__name__, a is b)


def t_ba_imul_idx_zero():
    a = bytearray(b"ab")
    b = a
    a *= Idx(0)
    return (a, a is b)


def t_basub_imul_idx_neg():
    a = BASub(b"ab")
    b = a
    a *= Idx(-1)
    return (a, type(a).__name__, a is b)


def t_ba_imul_boom():
    a = bytearray(b"ab")
    try:
        a *= IdxBoom()
    except ValueError as e:
        return (a, str(e))


def t_ba_imul_huge():
    a = bytearray(b"a")
    a *= IdxHuge()
    return a


def t_basub_imul_huge():
    a = BASub(b"a")
    a *= IdxHuge()
    return a


def t_ba_imul_bigint():
    a = bytearray(b"a")
    a *= 1 << 100
    return a


def t_ba_imul_memerr():
    a = bytearray(b"a")
    try:
        a *= 1 << 62
    except MemoryError:
        return ("MemoryError", a)


def t_ba_imul_hasrmul():
    a = bytearray(b"a")
    a *= HasRMul()
    return a


def t_basub_imul_hasrmul():
    a = BASub(b"a")
    a *= HasRMul()
    return a


def t_ba_imul_both():
    a = bytearray(b"a")
    a *= BothIndexAndRMul()
    return a


def t_basub_imul_both_ni():
    a = BASub(b"a")
    b = a
    a *= BothIndexAndRMulNI()
    return (a, type(a).__name__, a is b)


def t_ba_iadd_hasradd():
    a = bytearray(b"a")
    a += HasRAdd()
    return a


def t_basub_iadd_hasradd():
    a = BASub(b"a")
    a += HasRAdd()
    return a


def t_ba_iadd_iter_radd():
    a = bytearray(b"a")
    a += IterWithRAdd()
    return a


def t_ba_iadd_bsub_radd():
    a = bytearray(b"a")
    a += BSubRAdd(b"b")
    return a


check("7.1 ba *= Idx(3)", t_ba_imul_idx)
check("7.2 BASub *= Idx(3)", t_basub_imul_idx)
check("7.3 ba *= Idx(0)", t_ba_imul_idx_zero)
check("7.4 BASub *= Idx(-1)", t_basub_imul_idx_neg)
check("7.5 ba *= IdxBoom()", t_ba_imul_boom)
check("7.6 ba *= IdxHuge()", t_ba_imul_huge)
check("7.7 BASub *= IdxHuge()", t_basub_imul_huge)
check("7.8 ba *= (1<<100)", t_ba_imul_bigint)
check("7.9 ba *= (1<<62)", t_ba_imul_memerr)
check("7.10 ba *= HasRMul()", t_ba_imul_hasrmul)
check("7.11 BASub *= HasRMul()", t_basub_imul_hasrmul)
check("7.12 ba *= BothIndexAndRMul()", t_ba_imul_both)
check("7.13 BASub *= BothIndexAndRMulNI()", t_basub_imul_both_ni)
check("7.14 ba += HasRAdd()", t_ba_iadd_hasradd)
check("7.15 BASub += HasRAdd()", t_basub_iadd_hasradd)
check("7.16 ba += IterWithRAdd()", t_ba_iadd_iter_radd)
check("7.17 ba += BSubRAdd", t_ba_iadd_bsub_radd)
check("7.18 ba * Idx(3)", lambda: bytearray(b"a") * Idx(3))
check("7.19 Idx(3) * ba", lambda: Idx(3) * bytearray(b"a"))
check("7.20 ba * IdxHuge()", lambda: bytearray(b"a") * IdxHuge())
check("7.21 BASub * Idx(3)", lambda: (BASub(b"a") * Idx(3), type(BASub(b"a") * Idx(3)).__name__))
check("7.22 BASub * IdxHuge()", lambda: BASub(b"a") * IdxHuge())
check("7.23 BASub * HasRMul()", lambda: BASub(b"a") * HasRMul())
check("7.24 BASub * 'x'", lambda: BASub(b"a") * "x")
check("7.25 'x' * BASub", lambda: "x" * BASub(b"a"))
check("7.26 BASub + HasRAdd()", lambda: BASub(b"a") + HasRAdd())
check("7.27 BASub + 42", lambda: BASub(b"a") + 42)
check("7.28 'x' + BASub", lambda: "x" + BASub(b"a"))
check("7.29 42 + BASub", lambda: 42 + BASub(b"a"))
check("7.30 [1] + BASub", lambda: [1] + BASub(b"a"))
check("7.31 ba.__iadd__(HasRAdd)", lambda: bytearray(b"a").__iadd__(HasRAdd()))
check("7.32 ba.__imul__(Idx(3))", lambda: bytearray(b"a").__imul__(Idx(3)))
check("7.33 ba.__imul__(IdxHuge())", lambda: bytearray(b"a").__imul__(IdxHuge()))
check("7.34 ba.__mul__(IdxHuge())", lambda: bytearray(b"a").__mul__(IdxHuge()))
