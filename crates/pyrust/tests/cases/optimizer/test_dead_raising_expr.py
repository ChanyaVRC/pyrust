# Parity fixture for issue #2487: a function body whose only effect is a
# may-raise binary op with a DEAD result must still propagate its exception.
#
# Such a function used to be misclassified as "pure" by the CallMemo purity
# analysis, so a dead-result call to it was dead-store-eliminated by the
# optimizer — silently swallowing ZeroDivisionError / ValueError instead of
# raising.  The observable behaviour (output) must match CPython 3.12.


# --- constant divisor, dead result --------------------------------------------
def f_div_const():
    x = 1 / 0  # noqa: result unused but MUST raise


try:
    f_div_const()
    print("f_div_const: no error (WRONG)")
except ZeroDivisionError:
    print("f_div_const: ZeroDivisionError")


# --- bare expression statement, result discarded ------------------------------
def g_bare():
    1 / 0


try:
    g_bare()
    print("g_bare: no error (WRONG)")
except ZeroDivisionError:
    print("g_bare: ZeroDivisionError")


# --- runtime (non-const-foldable) divisor, dead result ------------------------
def h_div_runtime(d):
    x = 1 / d


try:
    h_div_runtime(0)
    print("h_div_runtime: no error (WRONG)")
except ZeroDivisionError:
    print("h_div_runtime: ZeroDivisionError")


# --- floor division and modulo by zero, dead result --------------------------
def i_floordiv(d):
    x = 1 // d


try:
    i_floordiv(0)
    print("i_floordiv: no error (WRONG)")
except ZeroDivisionError:
    print("i_floordiv: ZeroDivisionError")


def j_mod(d):
    x = 1 % d


try:
    j_mod(0)
    print("j_mod: no error (WRONG)")
except ZeroDivisionError:
    print("j_mod: ZeroDivisionError")


# --- pow that raises (0 ** negative), dead result -----------------------------
def k_pow():
    x = 0 ** -1


try:
    k_pow()
    print("k_pow: no error (WRONG)")
except ZeroDivisionError:
    print("k_pow: ZeroDivisionError")


# --- user-defined __pow__ with a side effect, dead result ---------------------
# The call must not be eliminated: the dunder has an observable side effect.
log = []


class Base:
    def __pow__(self, other):
        log.append("pow called")
        return 0


def l_user_pow():
    b = Base()
    x = b ** 2


l_user_pow()
print("l_user_pow log:", log)


# --- impure function call with a dead result ----------------------------------
def noisy():
    print("noisy ran")
    return 1


def m_calls_noisy():
    x = noisy()


m_calls_noisy()


# --- genuinely pure dead store is still fine (no error, no output) ------------
# These exercise the preserved optimization: a dead `1 + 2` may be eliminated;
# the only requirement is that nothing observable changes.
def n_pure_add():
    x = 1 + 2


n_pure_add()
print("n_pure_add: ok")


def o_pure_div_ok():
    x = 1 / 2  # non-zero divisor: no exception


o_pure_div_ok()
print("o_pure_div_ok: ok")
