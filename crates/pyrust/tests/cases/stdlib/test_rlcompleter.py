"""CPython 3.12 parity tests for the `rlcompleter` module (issue #3228)."""

import rlcompleter


# --- Public surface & validation ---
print("__all__:", rlcompleter.__all__)
print("Completer.__module__:", rlcompleter.Completer.__module__)

try:
    rlcompleter.Completer(123)
except TypeError as exc:
    print("non-dict namespace TypeError:", str(exc))

try:
    rlcompleter.Completer(["not", "a", "dict"])
except TypeError as exc:
    print("list namespace TypeError:", str(exc))


# --- Helper objects for completion tests ---
def fn_no_args():
    return 1


def fn_with_args(a, b=2):
    return a + b


class Base:
    base_val = 10

    def base_zero(self):
        return 0

    def base_one(self, x):
        return x


class Child(Base):
    child_val = 20
    _private = 1
    __secret = 2

    def __init__(self, name):
        self.name = name
        self.nested = Inner()

    def zero_method(self):
        return self.name

    def arg_method(self, y):
        return y

    @property
    def computed(self):
        raise RuntimeError("property must NOT be evaluated during completion!")


class Inner:
    leaf_num = 42

    def leaf_call(self):
        return 42

    def leaf_arg(self, z):
        return z


ns = {
    "fn_no_args": fn_no_args,
    "fn_with_args": fn_with_args,
    "Child": Child,
    "obj": Child("demo"),
    "my_var": 99,
    "try_custom": 123,
}

comp = rlcompleter.Completer(ns)


# --- 1. global_matches: keywords, soft keywords, namespace, builtins ---
print("kw 'tr':", sorted(comp.global_matches("tr")))
print("kw 'fin':", sorted(comp.global_matches("fin")))
print("kw 'impo':", sorted(comp.global_matches("impo")))
print("kw 'Fal':", sorted(comp.global_matches("Fal")))
print("kw 'Non':", sorted(comp.global_matches("Non")))
print("kw 'Tru':", sorted(comp.global_matches("Tru")))
print("kw 'pas':", sorted(comp.global_matches("pas")))
print("kw 'bre':", sorted(comp.global_matches("bre")))
print("kw 'matc':", sorted(comp.global_matches("matc")))
print("kw 'cas':", sorted(comp.global_matches("cas")))
print("kw 'typ':", sorted(comp.global_matches("typ")))
print("ns 'fn_':", sorted(comp.global_matches("fn_")))
print("ns 'my_':", sorted(comp.global_matches("my_")))
print("ns 'Child':", sorted(comp.global_matches("Child")))
print("builtin 'len':", "len(" in comp.global_matches("le"))
print("builtin 'abs':", "abs(" in comp.global_matches("ab"))
print("builtin 'dict':", "dict(" in comp.global_matches("di"))
print("builtin 'object':", "object()" in comp.global_matches("obj"))
print("no __builtins__:", "__builtins__" not in comp.global_matches("__"))


# --- 2. attr_matches: dotted paths, callables, @property guard, _ / __ hiding ---
obj_public = comp.attr_matches("obj.")
print("obj. has public attrs:", [
    m for m in obj_public
    if m in (
        "obj.arg_method(",
        "obj.base_one(",
        "obj.base_val",
        "obj.base_zero()",
        "obj.child_val",
        "obj.computed",
        "obj.name",
        "obj.nested",
        "obj.zero_method()",
    )
])
# Public listing hides leading-underscore attrs when public attrs exist
print("obj. hides _private:", "obj._private" not in obj_public)
print("obj. hides __class__:", "obj.__class__(" not in obj_public and "obj.__class__" not in obj_public)

# Single-underscore prefix reveals `_private` (and mangled `_Child__secret`) but hides `__dunder__`
obj_priv = comp.attr_matches("obj._")
print("obj._ has _private:", "obj._private" in obj_priv)
print("obj._ hides __init__:", not any(m.startswith("obj.__") for m in obj_priv))

# Double-underscore prefix reveals dunders
obj_dunder = comp.attr_matches("obj.__")
print("obj.__ has __class__:", any(m.startswith("obj.__class__") for m in obj_dunder))
print("obj.__ has __init__:", "obj.__init__(" in obj_dunder)

# Nested dotted path
print("obj.nested.leaf:", comp.attr_matches("obj.nested.leaf"))

# Class attributes & inheritance via get_class_members
cls_members = rlcompleter.get_class_members(Child)
print("get_class_members has base & child:", "base_val" in cls_members and "child_val" in cls_members)
print("Child.base:", comp.attr_matches("Child.base"))

# Invalid or non-evaluable expressions return []
print("attr bad syntax:", comp.attr_matches("obj..foo"))
print("attr missing root:", comp.attr_matches("does_not_exist.x"))
print("attr no dot:", comp.attr_matches("obj"))
print("attr call expr rejected:", comp.attr_matches("fn_no_args().x"))


# --- 3. complete(text, state) iteration ---
def collect_complete(completer, text):
    out = []
    state = 0
    while True:
        val = completer.complete(text, state)
        if val is None:
            break
        out.append(val)
        state += 1
    return out


print("complete 'fn_':", sorted(collect_complete(comp, "fn_")))
print("complete 'obj.nested.leaf':", collect_complete(comp, "obj.nested.leaf"))
print("complete nonexistent:", collect_complete(comp, "zzz_never_matches"))

# Blank text completion without readline returns '\t' on state=0, None on state=1
saved_rl_avail = rlcompleter._readline_available
try:
    rlcompleter._readline_available = False
    print("complete blank:", comp.complete("", 0), comp.complete("", 1))
    print("complete spaces:", comp.complete("   ", 0), comp.complete("   ", 1))
finally:
    rlcompleter._readline_available = saved_rl_avail


# --- 4. Default namespace binds lazily to __main__.__dict__ ---
main_comp = rlcompleter.Completer()
print("use_main_ns:", main_comp.use_main_ns)
main_sentinel_xyz = 777
print("main sentinel found:", "main_sentinel_xyz" in collect_complete(main_comp, "main_sentinel_"))

print("rlcompleter parity ok")
