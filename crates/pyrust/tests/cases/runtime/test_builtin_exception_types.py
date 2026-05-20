# Parity fixture for issue #339: assert the Python class of exceptions raised
# by built-in operations.  Each check() call prints one line: "label ClassName"
# so the harness diffs stdout between CPython 3.12 and pyrust.

def check(name, fn):
    try:
        fn()
    except Exception as e:
        print(name, type(e).__name__)

# --- Container access ---
check("dict_missing",      lambda: {}["x"])
check("dict_pop_missing",  lambda: {}.pop("x"))
check("list_oob",          lambda: [][0])
check("list_pop_empty",    lambda: [].pop())
check("tuple_oob",         lambda: ()[0])
check("str_oob",           lambda: ""[0])

# --- Type errors from operators ---
_d = {}
_i = 1
check("dict_add",          lambda: _d + _d)
check("str_add_int",       lambda: "" + 1)
check("not_subscriptable", lambda: _i[0])
check("not_callable",      lambda: _i(2))
check("not_iterable",      lambda: list(_i))
check("unhashable_key",    lambda: {[1]: 2})

# --- Name / attribute lookup ---
check("undef_name",        lambda: undef_name)
check("none_attr",         lambda: None.attr)
check("missing_attr",      lambda: _i.nope)

# --- Arithmetic ---
check("zero_div_int",      lambda: 1 / 0)
check("zero_div_mod",      lambda: 1 % 0)
check("zero_div_floor",    lambda: 1 // 0)

# --- Conversion ---
check("int_bad_literal",   lambda: int("abc"))

# --- len() on non-sized ---
check("len_non_sized",     lambda: len(_i))

# --- Call-signature errors ---
def _f0(): pass
def _g1(x): pass

check("too_many_args",     lambda: _f0(1))
check("too_few_args",      lambda: _g1())
check("dup_kwarg",         lambda: _g1(1, x=2))
