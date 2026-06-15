# Parity fixture for issue #2517: a function body whose only effect is an
# attribute access / subscription / slice with a DEAD result must still
# propagate its exception.
#
# Such a function used to be misclassified as "pure" by the CallMemo purity
# analysis (is_pure_expr recursed into the target/index and returned true for
# `(1).foo`, `{}["x"]`, etc.), so a dead-result call to it was dead-store-
# eliminated by the optimizer — silently swallowing AttributeError / KeyError /
# TypeError instead of raising.  Sibling of #2409 (`raise`) and #2487 (div/mod).
# The observable behaviour (output) must match CPython 3.12.


# --- attribute access on a literal, dead result -------------------------------
def f_attr():
    x = (1).foo  # noqa: result unused but MUST raise AttributeError


try:
    f_attr()
    print("f_attr: no error (WRONG)")
except AttributeError:
    print("f_attr: AttributeError")


# --- bare attribute expression statement, result discarded --------------------
def g_attr_bare():
    (1).foo


try:
    g_attr_bare()
    print("g_attr_bare: no error (WRONG)")
except AttributeError:
    print("g_attr_bare: AttributeError")


# --- subscription raising KeyError, dead result -------------------------------
def h_index_key():
    x = {}["x"]


try:
    h_index_key()
    print("h_index_key: no error (WRONG)")
except KeyError:
    print("h_index_key: KeyError")


# --- subscription on a non-subscriptable int, dead result ---------------------
# A runtime int (not a literal) avoids CPython's compile-time SyntaxWarning
# while still raising the same TypeError at runtime.
def i_index_type():
    n = 1
    x = n[0]


try:
    i_index_type()
    print("i_index_type: no error (WRONG)")
except TypeError:
    print("i_index_type: TypeError")


# --- slice on a non-subscriptable int, dead result ----------------------------
def j_slice_type():
    n = 1
    x = n[0:1]


try:
    j_slice_type()
    print("j_slice_type: no error (WRONG)")
except TypeError:
    print("j_slice_type: TypeError")


# --- interpolated f-string with a bad format spec, dead result ----------------
# Sibling of the attr/index cases: an f-string with an interpolated expression
# invokes the formatting protocol, and a bad spec on a built-in raises
# ValueError.  All operands here are literals (pure), so the function used to be
# classified pure and the dead-result f-string was eliminated, swallowing the
# error.  Must still raise.
def f_fstring_spec():
    x = f"{(1):foo}"  # noqa: result unused but MUST raise ValueError


try:
    f_fstring_spec()
    print("f_fstring_spec: no error (WRONG)")
except ValueError:
    print("f_fstring_spec: ValueError")


# --- user __format__ with a side effect via f-string, dead result -------------
fmt_log = []


class Formatted:
    def __format__(self, spec):
        fmt_log.append(("format", spec))
        return ""


def f_fstring_user():
    obj = Formatted()
    x = f"{obj:zz}"


f_fstring_user()
print("f_fstring_user log:", fmt_log)


# --- user-defined __getattr__ with a side effect, dead result -----------------
# The call must not be eliminated: the dunder has an observable side effect.
log = []


class Watcher:
    def __getattr__(self, name):
        log.append(("getattr", name))
        return 0

    def __getitem__(self, key):
        log.append(("getitem", key))
        return 0


def k_user_getattr():
    w = Watcher()
    x = w.anything


k_user_getattr()
print("k_user_getattr log:", log)


def l_user_getitem():
    w = Watcher()
    x = w["k"]


l_user_getitem()
print("l_user_getitem log:", log)


# --- genuinely pure dead store / pure-call chain still works ------------------
# Exercises the preserved optimization: a function that only does pure work and
# calls another pure function may still be memoized/eliminated; the only
# requirement is that nothing observable changes.
def add_one(v):
    return v + 1


def m_pure_chain(v):
    return add_one(v) * 2


print("m_pure_chain:", m_pure_chain(10))
for _ in range(3):
    m_pure_chain(10)
print("m_pure_chain: ok")
