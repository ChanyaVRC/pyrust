# Repr of built-in iterators, generators, itertools.count/repeat, and
# functools.partial (#2016, #2019, #2020).
#
# Address-stability: iterator / generator / partial reprs embed a heap
# address (`at 0x...`) that varies per run, so this fixture normalises every
# `0x...` run to `0xADDR` before printing.  The diff against CPython therefore
# asserts the *structure* (type name, qualname, args), never the raw hex.
import itertools
import functools

_HEX = "0123456789abcdefABCDEF"


def _strip_addr(s):
    # Replace every `0x<hex>` run with the stable token `0xADDR` without
    # depending on the `re` module (pyrust's stdlib does not ship `re`).
    out = []
    i = 0
    while i < len(s):
        if s[i : i + 2] == "0x":
            j = i + 2
            while j < len(s) and s[j] in _HEX:
                j += 1
            out.append("0xADDR")
            i = j
        else:
            out.append(s[i])
            i += 1
    return "".join(out)


def norm(obj):
    # Collapse any hex address to a stable token so the parity diff is
    # deterministic across runs and interpreters.
    return _strip_addr(repr(obj))


def norm_func(obj):
    # As `norm`, but also strips the embedded function address clause
    # ` at 0x...>` down to `>`.  pyrust's function repr is `<function f>`
    # while CPython's is `<function f at 0x...>`; removing the address clause
    # makes the partial repr structure comparable across both.
    return _strip_addr(repr(obj)).replace(" at 0xADDR>", ">")


# ── #2019: built-in iterators ────────────────────────────────────────────────
print(norm(map(str, [1])))
print(norm(filter(None, [1])))
print(norm(zip([1], [2])))
print(norm(enumerate([1])))
print(norm(iter([1])))
print(norm(iter((1,))))
print(norm(iter("a")))      # ascii -> str_ascii_iterator
print(norm(iter("é")))  # non-ascii -> str_iterator
print(norm(iter(range(1))))
print(norm(iter({1})))
print(norm(iter({1: 2})))

# ── #2019: generators (genexpr keeps <genexpr>, def-gen keeps its qualname) ──
print(norm(x for x in [1]))


def my_gen():
    yield 1


print(norm(my_gen()))

# Generator __name__ / __qualname__ track the function name (genexpr -> <genexpr>).
print((x for x in [1]).__name__)
print((x for x in [1]).__qualname__)
print(my_gen().__name__)

# ── #2016: itertools.count (step omitted only for the default int 1) ─────────
print(repr(itertools.count(5)))
print(repr(itertools.count(2, 3)))
print(repr(itertools.count()))
print(repr(itertools.count(0, -1)))
print(repr(itertools.count(0, 1)))     # int step 1 -> omitted
print(repr(itertools.count(0, True)))  # bool True == 1 -> omitted
print(repr(itertools.count(0, 1.0)))   # float 1.0 -> shown
print(repr(itertools.count(5, 2)))     # non-default step -> shown
print(repr(itertools.count(2.5)))

# count repr reflects the advancing current value.
c = itertools.count(10)
next(c)
print(repr(c))

# ── #2020: itertools.repeat (times shown only when bounded; tracks remaining) ─
print(repr(itertools.repeat(5, 3)))
print(repr(itertools.repeat("x")))
print(repr(itertools.repeat(5, 0)))
r = itertools.repeat(7, 3)
next(r)
print(repr(r))  # remaining count decremented

# ── #2020: functools.partial (embedded func repr normalised) ─────────────────
def f(a, b):
    pass


print(norm_func(functools.partial(f, 1)))
print(norm_func(functools.partial(f, 1, c=3)))
print(norm_func(functools.partial(f, 1, 2, x=3, y=4)))
