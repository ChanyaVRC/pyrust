import _symtable
import symtable

# 1. Constants parity
print(
    "constants:",
    _symtable.DEF_GLOBAL,
    _symtable.DEF_LOCAL,
    _symtable.DEF_PARAM,
    _symtable.DEF_NONLOCAL,
    _symtable.USE,
    _symtable.DEF_IMPORT,
    _symtable.DEF_ANNOT,
    _symtable.DEF_BOUND,
    _symtable.LOCAL,
    _symtable.GLOBAL_EXPLICIT,
    _symtable.GLOBAL_IMPLICIT,
    _symtable.FREE,
    _symtable.CELL,
    _symtable.SCOPE_OFF,
    _symtable.SCOPE_MASK,
)

# 2. Comprehensive lexical scoping, nested functions, closures, classes, PEP 709
src = """
import os
from sys import path as sp
a: int = 1
b: str

def outer(p, *args, kw=1, **kwargs):
    global g_decl
    g_decl = 2
    loc = p + g_impl
    lc = [i for i in range(3)]
    ge = (j for j in range(3))
    def inner():
        nonlocal loc
        loc += 1
        return p + g_decl + g_impl
    return inner

class Foo:
    x = 1
    __priv = 2
    def bar(self):
        return self.__priv
    async def abaz(self):
        pass
"""

st = symtable.symtable(src, "sample.py", "exec")
print(
    "top:",
    repr(st),
    st.get_type(),
    st.get_name(),
    st.get_lineno(),
    st.is_optimized(),
    st.is_nested(),
    st.has_children(),
    isinstance(st.get_id(), int),
)
print("top_idents:", list(st.get_identifiers()))

for name in ["os", "sp", "a", "b", "outer", "g_decl", "Foo"]:
    s = st.lookup(name)
    print(
        f"top_sym[{name}]:",
        repr(s),
        s.get_name(),
        s.is_referenced(),
        s.is_imported(),
        s.is_parameter(),
        s.is_global(),
        s.is_declared_global(),
        s.is_nonlocal(),
        s.is_local(),
        s.is_annotated(),
        s.is_free(),
        s.is_assigned(),
        s.is_namespace(),
        len(s.get_namespaces()),
    )

# get_namespace() single vs zero
outer_sym = st.lookup("outer")
outer_tbl = outer_sym.get_namespace()
print(
    "outer:",
    isinstance(outer_tbl, symtable.Function),
    isinstance(outer_tbl, symtable.SymbolTable),
    repr(outer_tbl),
    outer_tbl.get_type(),
    outer_tbl.get_name(),
    outer_tbl.get_lineno(),
    outer_tbl.is_optimized(),
    outer_tbl.is_nested(),
    outer_tbl.has_children(),
)
print(
    "outer_sets:",
    outer_tbl.get_parameters(),
    outer_tbl.get_locals(),
    outer_tbl.get_globals(),
    outer_tbl.get_nonlocals(),
    outer_tbl.get_frees(),
)

# Zero namespaces -> ValueError
try:
    st.lookup("a").get_namespace()
except ValueError as exc:
    print("zero_ns_err:", str(exc))

# Lookup missing -> KeyError
try:
    st.lookup("does_not_exist")
except KeyError:
    print("missing_key_err: ok")

# Children of outer: genexpr + inner
outer_children = outer_tbl.get_children()
print("outer_children:", [(c.get_name(), c.get_type(), c.is_nested()) for c in outer_children])
inner_tbl = outer_tbl.lookup("inner").get_namespace()
print(
    "inner_sets:",
    inner_tbl.get_parameters(),
    inner_tbl.get_locals(),
    inner_tbl.get_globals(),
    inner_tbl.get_nonlocals(),
    inner_tbl.get_frees(),
)
loc_sym = inner_tbl.lookup("loc")
print(
    "inner_loc:",
    loc_sym.is_nonlocal(),
    loc_sym.is_free(),
    loc_sym.is_assigned(),
    loc_sym.is_local(),
    loc_sym.is_global(),
)

# Class table & methods & private name mangling
foo_tbl = st.lookup("Foo").get_namespace()
print(
    "foo:",
    isinstance(foo_tbl, symtable.Class),
    repr(foo_tbl),
    foo_tbl.get_type(),
    foo_tbl.get_name(),
    foo_tbl.is_optimized(),
    foo_tbl.get_methods(),
    "_Foo__priv" in foo_tbl.get_identifiers(),
)

# Eval mode
eval_st = symtable.symtable("x + y", "<expr>", "eval")
print("eval:", eval_st.get_type(), sorted(eval_st.get_identifiers()))

# Invalid mode -> ValueError
try:
    symtable.symtable("x = 1", "<bad>", "bogus")
except ValueError:
    print("invalid_mode: ok")
