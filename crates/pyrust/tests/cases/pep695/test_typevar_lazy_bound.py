# PEP 695 (#2290): a type parameter's bound (`T: int`) and constraints
# (`T: (int, str)`) are evaluated *lazily* — on first access of `__bound__` /
# `__constraints__`, not at def/class/alias time — matching CPython's deferred
# annotation scope.  The result is cached on first access, and any exception
# raised by the clause propagates from the access, not the definition.

# ── Lazy: the bound is not evaluated at def time ──────────────────────────────

log = []


def mk_bound():
    log.append("bound-eval")
    return int


def f[T: mk_bound()](x):
    return x


print("f defined, log =", log)              # f defined, log = []
T = f.__type_params__[0]
print("before access, log =", log)          # before access, log = []
b1 = T.__bound__
print("after access, log =", log)           # after access, log = ['bound-eval']
print("bound =", b1)                        # bound = <class 'int'>

# ── Cached: a second access reuses the first result (same object, no re-eval) ─

b2 = T.__bound__
print("cached same object:", b1 is b2)      # cached same object: True
print("not re-evaluated, log =", log)       # not re-evaluated, log = ['bound-eval']

# ── Lazy constraints behave the same way ─────────────────────────────────────

clog = []


def mk_c1():
    clog.append("c1")
    return int


def mk_c2():
    clog.append("c2")
    return str


def g[T: (mk_c1(), mk_c2())](x):
    return x


print("g defined, clog =", clog)            # g defined, clog = []
G = g.__type_params__[0]
c1 = G.__constraints__
print("constraints =", c1)                  # constraints = (<class 'int'>, <class 'str'>)
c2 = G.__constraints__
print("constraints cached:", c1 is c2)      # constraints cached: True
print("clog =", clog)                       # clog = ['c1', 'c2']

# ── Exception during bound evaluation propagates on access, not at def ────────

def undef_bound[T: undefined_name](x):
    return x


print("undef_bound defined ok")             # undef_bound defined ok
U = undef_bound.__type_params__[0]
try:
    U.__bound__
except NameError as e:
    print("NameError on access:", e)        # NameError on access: name 'undefined_name' is not defined

# ── A name defined *later* in the module resolves on access ───────────────────

def later[T: LaterDefined](x):
    return x


print("later defined ok")                   # later defined ok


class LaterDefined:
    pass


print("later bound resolves:", later.__type_params__[0].__bound__ is LaterDefined)  # True

# ── The internal evaluation thunk slot is not observable ─────────────────────

def h[T: int](x):
    return x


H = h.__type_params__[0]
print("eval slot hidden:", hasattr(H, "__evaluate_bound__"))  # eval slot hidden: False

# ── Type alias: lazy + forward/self reference ────────────────────────────────

ylog = []


def mk_alias_bound():
    ylog.append("alias-bound")
    return int


type AliasLazy[T: mk_alias_bound()] = list[T]
print("alias defined, ylog =", ylog)        # alias defined, ylog = []
A = AliasLazy.__type_params__[0]
print("alias bound =", A.__bound__)         # alias bound = <class 'int'>
print("alias bound re-eval, ylog =", ylog)  # alias bound re-eval, ylog = ['alias-bound']

type FwdAlias[T, U: T] = list[U]
print("forward bound name:", FwdAlias.__type_params__[1].__bound__.__name__)  # T
print(
    "forward bound identity:",
    FwdAlias.__type_params__[1].__bound__ is FwdAlias.__type_params__[0],
)  # forward bound identity: True

# ── Generic class: lazy bound, name defined later ────────────────────────────

class C[T: LaterC]:
    pass


print("class defined ok")                   # class defined ok


class LaterC:
    pass


print("class bound resolves:", C.__type_params__[0].__bound__ is LaterC)  # True

# ── Bare parameter keeps the eager defaults ──────────────────────────────────

def bare[T](x):
    return x


B = bare.__type_params__[0]
print("bare bound:", B.__bound__)           # bare bound: None
print("bare constraints:", B.__constraints__)  # bare constraints: ()

# ── Same-site repeated access across params (GetAttr inline-cache guard) ───────
# `for t in __type_params__: t.__bound__` reads `__bound__` from one bytecode
# site for several distinct TypeVars.  The lazy thunk lives behind the slow-path
# interceptor, but an eager None/() default also sits in the instance dict, so if
# the GetAttr inline cache caches the first param's instance-attr read, later
# params would wrongly return that stale default and skip their thunk.


def multi[A, B: int, C: (str, bytes), D](x):
    return x


for t in multi.__type_params__:
    print("multi:", t.__name__, t.__bound__, t.__constraints__)
# multi: A None ()
# multi: B <class 'int'> ()
# multi: C None (<class 'str'>, <class 'bytes'>)
# multi: D None ()
