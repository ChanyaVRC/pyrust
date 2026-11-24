# Parity fixture for issue #2971:
#   1. No dotted internal registry keys in vars(builtins) / builtins.__dict__ / dir(builtins)
#   2. del module.__doc__ fallback to type(module).__doc__ & types.ModuleType.__doc__
#   3. Imported-module __dict__ tail (__file__, __cached__, __builtins__, lazy __annotations__)
#   4. vars(1) exact CPython 3.12 TypeError wording
#   5. Read-only module.__dict__ across built-in, sys, and imported modules

import builtins
import math
import sys
import types

import _mod_2971_annotated as mod_ann
import _mod_2971_plain as mod_plain

PLAIN_INITIAL_KEYS = list(vars(mod_plain).keys())
ANN_INITIAL_KEYS = list(vars(mod_ann).keys())


def show(label, fn):
    try:
        res = fn()
        print(f"{label}: OK {res!r}")
    except BaseException as exc:
        print(f"{label}: EXC {type(exc).__name__} {exc.args!r}")


# =============================================================================
# 1. No dotted internal registry keys in builtins
# =============================================================================

print("1.dotted in vars(builtins):", [k for k in vars(builtins) if "." in k])
print("1.dotted in builtins.__dict__:", [k for k in builtins.__dict__ if "." in k])
print("1.dotted in dir(builtins):", [k for k in dir(builtins) if "." in k])
print("1.hasattr list.__init__:", hasattr(builtins, "list.__init__"))
show("1.getattr list.__init__", lambda: getattr(builtins, "list.__init__"))
print("1.core builtins present:", all(hasattr(builtins, n) for n in ("abs", "len", "int", "list", "ValueError", "None", "__debug__")))


# =============================================================================
# 2. del module.__doc__ fallback to type(module).__doc__ & types.ModuleType.__doc__
# =============================================================================

print("2.type(math) is ModuleType:", type(math) is types.ModuleType)
print("2.type(math).__doc__:", repr(type(math).__doc__))
print("2.ModuleType.__doc__:", repr(types.ModuleType.__doc__))
print("2.same doc:", type(math).__doc__ == types.ModuleType.__doc__)

# Built-in module (math): del __doc__ falls back to type(math).__doc__
saved_math_doc = math.__doc__
del math.__doc__
print("2.math __doc__ in vars after del:", "__doc__" in vars(math))
print("2.math.__doc__ after del:", repr(math.__doc__))
show("2.math second del __doc__", lambda: delattr(math, "__doc__"))
math.__doc__ = saved_math_doc

# Live-namespace module (sys): del __doc__ falls back to type(sys).__doc__
saved_sys_doc = sys.__doc__
del sys.__doc__
print("2.sys __doc__ in vars after del:", "__doc__" in vars(sys))
print("2.sys.__doc__ after del:", repr(sys.__doc__))
show("2.sys second del __doc__", lambda: delattr(sys, "__doc__"))
sys.__doc__ = saved_sys_doc

# Imported filesystem module: del __doc__ falls back to type(mod).__doc__
saved_plain_doc = mod_plain.__doc__
print("2.mod_plain initial __doc__:", repr(saved_plain_doc))
del mod_plain.__doc__
print("2.mod_plain __doc__ in vars after del:", "__doc__" in vars(mod_plain))
print("2.mod_plain.__doc__ after del:", repr(mod_plain.__doc__))
show("2.mod_plain second del __doc__", lambda: delattr(mod_plain, "__doc__"))
mod_plain.__doc__ = saved_plain_doc

# Deleting __name__ still raises AttributeError (no type fallback)
saved_math_name = math.__name__
del math.__name__
show("2.math read after del __name__", lambda: math.__name__)
show("2.math second del __name__", lambda: delattr(math, "__name__"))
math.__name__ = saved_math_name


# =============================================================================
# 3. Imported-module __dict__ tail & lazy __annotations__
# =============================================================================

# Top-level __main__ script keeps CPython's __main__ dunder head order
main_head = [
    "__name__",
    "__doc__",
    "__package__",
    "__loader__",
    "__spec__",
    "__annotations__",
    "__builtins__",
    "__file__",
    "__cached__",
]
print("3.main globals head:", list(globals().keys())[:9] == main_head)

# Imported plain module (no module-level annotations):
# __file__, __cached__, __builtins__ (NO eager __annotations__)
plain_expected = [
    "__name__",
    "__doc__",
    "__package__",
    "__loader__",
    "__spec__",
    "__file__",
    "__cached__",
    "__builtins__",
    "VAL",
    "check_bare_annotations",
    "fn_with_local_ann",
]
print("3.mod_plain exact initial keys:", PLAIN_INITIAL_KEYS == plain_expected, PLAIN_INITIAL_KEYS)
print("3.mod_ann exact keys:", ANN_INITIAL_KEYS)
print("3.mod_ann __annotations__:", mod_ann.__annotations__)
print("3.mod_plain bare __annotations__:", mod_plain.check_bare_annotations())
print("3.mod_plain has __annotations__ in vars:", "__annotations__" in vars(mod_plain))
print("3.mod_plain fn_with_local_ann:", mod_plain.fn_with_local_ann(3))

# Lazy __annotations__ on imported module without annotations
show("3.del absent mod_plain.__annotations__", lambda: delattr(mod_plain, "__annotations__"))
a1 = mod_plain.__annotations__
a2 = mod_plain.__annotations__
print("3.mod_plain lazy __annotations__:", a1, a1 is a2, "__annotations__" in vars(mod_plain))
del mod_plain.__annotations__
print("3.mod_plain after del in vars:", "__annotations__" in vars(mod_plain))
a3 = mod_plain.__annotations__
print("3.mod_plain recreated distinct:", a3, a3 is not a1)

# Lazy __annotations__ on built-in module (math)
print("3.math has __annotations__ initially:", "__annotations__" in vars(math))
show("3.del absent math.__annotations__", lambda: delattr(math, "__annotations__"))
ma1 = math.__annotations__
ma2 = math.__annotations__
print("3.math lazy __annotations__:", ma1, ma1 is ma2, "__annotations__" in vars(math))
del math.__annotations__
print("3.math after del in vars:", "__annotations__" in vars(math))
ma3 = math.__annotations__
print("3.math recreated distinct:", ma3, ma3 is not ma1)
del math.__annotations__


# =============================================================================
# 4. vars(1) exact TypeError wording
# =============================================================================

show("4.vars(1)", lambda: vars(1))
show("4.vars('abc')", lambda: vars("abc"))
show("4.vars([])", lambda: vars([]))
show("4.vars(None)", lambda: vars(None))


# =============================================================================
# 6. Read-only module.__dict__ across built-in, sys, and imported modules
# =============================================================================

show("6.setattr math.__dict__", lambda: setattr(math, "__dict__", 5))
show("6.delattr math.__dict__", lambda: delattr(math, "__dict__"))
show("6.setattr sys.__dict__", lambda: setattr(sys, "__dict__", 5))
show("6.delattr sys.__dict__", lambda: delattr(sys, "__dict__"))
show("6.setattr mod_plain.__dict__", lambda: setattr(mod_plain, "__dict__", 5))
show("6.delattr mod_plain.__dict__", lambda: delattr(mod_plain, "__dict__"))
show("6.setattr mod_ann.__dict__", lambda: setattr(mod_ann, "__dict__", 5))
show("6.delattr mod_ann.__dict__", lambda: delattr(mod_ann, "__dict__"))
