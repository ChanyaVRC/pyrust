# functools.partial / lru_cache / wraps / cached_property — added in
# #329.  The four entries that didn't make it into the phase-2 stdlib
# drop.  pyrust's implementations are intentionally minimal but match
# CPython for the cases this script exercises (see issue #329).

import functools
from functools import partial, lru_cache, wraps, cached_property


# ── partial ──────────────────────────────────────────────────────────

def add(a, b, c=0):
    return a + b + c

# Positional pre-bind: partial pre-binds the first arg; call-site
# positionals fill in the rest left-to-right.
print("partial-pos", partial(add, 1)(2, 3))     # 6

# Kwarg pre-bind: only positional args at call site.
print("partial-kw", partial(add, c=10)(1, 2))   # 13

# Caller-wins override: pre-bound kwarg replaced by call-site kwarg.
print("partial-override", partial(add, 1, c=10)(2, c=99))  # 1+2+99 = 102


# ── lru_cache (bare and parenthesised forms) ─────────────────────────

# Bare `@lru_cache` form — applies defaults (maxsize=128, typed=False).
# Use a 1-element list as a call counter (pyrust's `+= 1` on dict
# subscript inside a function hits an interpreter bug, but list
# index assignment works fine).
calls = [0]

@lru_cache
def fib(n):
    calls[0] = calls[0] + 1
    return n if n < 2 else fib(n - 1) + fib(n - 2)

print("fib-10", fib(10))                # 55
# 11 unique calls (0..10); without the cache fib(10) would explode to ~177.
print("fib-call-count", calls[0])       # 11


# Parenthesised form: returns a decorator factory.
calls2 = [0]

@lru_cache(maxsize=2)
def square(x):
    calls2[0] = calls2[0] + 1
    return x * x

# Three distinct keys with maxsize=2 — the third call evicts the LRU.
square(1)
square(2)
square(3)
print("square-after-3", calls2[0])      # 3 (no cache hits yet)
# Re-call `1` — was evicted, recomputes.
square(1)
print("square-after-1-evict", calls2[0])  # 4
# Re-call `3` — still in cache, no recompute.
square(3)
print("square-after-3-hit", calls2[0])    # 4


# `typed=True` keeps int and float distinct.
calls3 = [0]

@lru_cache(typed=True)
def identity(x):
    calls3[0] = calls3[0] + 1
    return x

identity(1)
identity(1.0)
print("typed-distinct-calls", calls3[0])  # 2 — `1` and `1.0` differ


# `typed=False` (default): CPython *still* distinguishes `1` and
# `1.0` because its `_make_key` wraps non-int fast-types in a
# `_HashedSeq` whose hash never matches the bare-int key — so the
# call count is 2, not 1.  pyrust matches that.
calls4 = [0]

@lru_cache
def identity2(x):
    calls4[0] = calls4[0] + 1
    return x

identity2(1)
identity2(1.0)
print("untyped-int-vs-float", calls4[0])  # 2


# ── wraps ────────────────────────────────────────────────────────────

def original(x):
    """The original docstring."""
    return x * 2

@wraps(original)
def wrapper(x):
    return original(x) + 1

# `wrapper.__name__` after `@wraps(original)` reflects the original's
# name.  pyrust's minimal `wraps` synthesises a wrapper object that
# carries `__name__` (and `__doc__`) from the original.
print("wraps-name", wrapper.__name__)   # "original"
# Wrapper still callable, delegates to the wrapped function.
print("wraps-call", wrapper(5))         # 5*2+1 = 11


# ── cached_property ──────────────────────────────────────────────────
#
# pyrust currently can't see names imported from a module as decorators
# inside a class body (`@property` works because it's a global builtin;
# `@cached_property` doesn't because it's a regular module attribute).
# CPython, conversely, *requires* that `cached_property` be attached via
# the class-body decorator (or with an explicit `__set_name__` call)
# so it can capture the attribute name.  The portable form below
# constructs the descriptor outside the class body, attaches it via
# direct assignment, and calls `__set_name__` explicitly — works in
# both Python and pyrust.

class Counter:
    def __init__(self):
        self.runs = 0

    def expensive_impl(self):
        self.runs = self.runs + 1
        return self.runs * 100

Counter.expensive = cached_property(Counter.expensive_impl)
Counter.expensive.__set_name__(Counter, "expensive")

# First access computes; subsequent accesses re-use the cached value.
c = Counter()
print("cached-first", c.expensive)      # runs=1 → 100
print("cached-second", c.expensive)     # cached → still 100
print("cached-runs", c.runs)            # 1 (only one compute)

# Per-instance independence: two instances cache independently.
c1 = Counter()
c2 = Counter()
print("cached-c1", c1.expensive)        # 100 (its own runs=1)
print("cached-c2", c2.expensive)        # 100 (its own runs=1)
print("cached-c1-runs", c1.runs)        # 1
print("cached-c2-runs", c2.runs)        # 1

# `functools` exposes all four names at the module level — sanity
# check that the import paths are wired up.
print(
    "exposed",
    callable(functools.partial),
    callable(functools.lru_cache),
    callable(functools.wraps),
    callable(functools.cached_property),
)

# ── self-review fixes (PR #343 review comment) ────────────────────────


# `typed=False`: numerically-equal int and float compare equal even
# when stored in distinct cache slots — both CPython and pyrust use
# separate slots here (CPython's `_make_key` returns the bare int via
# its `fasttypes` fast path but wraps the float in a `_HashedSeq`, so
# the keys never collide), but the *return values* still compare
# equal under `==`.  We can't easily expose hit/miss without
# `cache_info()`, so we assert return-value equality only.  The real
# validation is the parity diff: CPython and pyrust both print the
# same thing here.
def _f(x):
    return x


cached_f = lru_cache()(_f)
_ = cached_f(1)        # miss → cache
_ = cached_f(1.0)      # miss → cache (separate slot in both runtimes)
print("lru-int-float-share", cached_f(1) == cached_f(1.0) == 1)

# Private constructors reject user args.  `_lru_cache_wrapper` is the
# inner type produced by `lru_cache()`; it's not exported by name but
# is reachable via `type(cached_f)`.  Calling it directly with user
# arguments should fail with TypeError rather than silently producing
# a broken instance.
try:
    inner_type = type(cached_f)
    _ = inner_type("bogus", "args")
    print("lru-private-init", "FAIL-no-error")
except TypeError:
    print("lru-private-init", "TypeError")


# ── self-review Copilot fixes (PR #343 review) ────────────────────────

# partial(func=...) raises TypeError — func is positional-only.
try:
    functools.partial(func=lambda x: x)(1)
    print("partial-kw-func", "FAIL-no-error")
except TypeError:
    print("partial-kw-func", "TypeError")

# cached_property.__set_name__ records the attr_name and __get__ stashes
# under that name (not the access-site name or the wrapped function's
# own name).  We can't reach `h.__dict__` directly in pyrust yet, so we
# inspect via `vars(h)` — that returns the instance dict in both
# runtimes.
class HasOffName:
    def _compute(self):
        return 42


desc = functools.cached_property(HasOffName._compute)
desc.__set_name__(HasOffName, "answer")
HasOffName.answer = desc
h = HasOffName()
v = h.answer
assert v == 42
d = vars(h)
assert "answer" in d
assert "_compute" not in d  # stashed under the set_name, not func name
print("cached-property-set-name", "ok")
