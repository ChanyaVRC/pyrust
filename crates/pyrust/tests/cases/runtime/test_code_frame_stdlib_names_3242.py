# CPython 3.12 parity tests for issue #3242:
# FrameType (f_builtins, f_lasti, frame.clear()),
# CodeType (co_lines, co_positions, co_linetable, co_lnotab, co_exceptiontable, code.replace),
# and sys.stdlib_module_names.

import builtins
import sys
import warnings

# 1. FrameType.f_builtins (must be actual builtins dict, NOT f_globals)
f_mod = sys._getframe()
print("f_builtins is dict:", isinstance(f_mod.f_builtins, dict))
print("f_builtins is not f_globals:", f_mod.f_builtins is not f_mod.f_globals)
print("f_builtins has len:", "len" in f_mod.f_builtins, f_mod.f_builtins["len"] is len)
print("f_builtins is builtins.__dict__:", f_mod.f_builtins is builtins.__dict__)

# Custom __builtins__ dict in exec()
custom_b = {"getframe": sys._getframe, "len": len}
ns_custom = {"__builtins__": custom_b}
exec("fr = getframe()", ns_custom)
print("custom f_builtins identity:", ns_custom["fr"].f_builtins is custom_b)

# 2. FrameType.f_lasti
print("module f_lasti non-negative:", isinstance(f_mod.f_lasti, int), f_mod.f_lasti >= 0)

def gen():
    x = 1
    yield x
    y = 2
    yield y

g = gen()
print("unstarted gen f_lasti is int:", isinstance(g.gi_frame.f_lasti, int))
next(g)
print("suspended gen f_lasti >= 0:", isinstance(g.gi_frame.f_lasti, int), g.gi_frame.f_lasti >= 0)
g.close()

# 3. FrameType.clear()
def capture_frame():
    local_a = 123
    local_b = "abc"
    fr = sys._getframe()
    try:
        fr.clear()
        print("clear active: NO ERROR")
    except RuntimeError as e:
        print("clear active:", type(e).__name__, str(e))
    return fr

inactive = capture_frame()
print("before clear has locals:", "local_a" in inactive.f_locals, "local_b" in inactive.f_locals)
inactive.clear()
print("after clear empty:", inactive.f_locals == {})

# 4. CodeType parity: co_lines(), co_positions(), co_linetable, co_lnotab, co_exceptiontable, replace()
def sample(a, b=2):
    c = a + b
    d = c * 3
    return d

co = sample.__code__
lines_it = co.co_lines()
print("co_lines is iterator:", iter(lines_it) is lines_it)
lines_list = list(lines_it)
print("co_lines non-empty:", len(lines_list) > 0)
print(
    "co_lines entry shapes:",
    all(
        isinstance(t, tuple)
        and len(t) == 3
        and isinstance(t[0], int)
        and isinstance(t[1], int)
        and t[0] >= 0
        and t[1] >= t[0]
        and (t[2] is None or isinstance(t[2], int))
        for t in lines_list
    ),
)
# Ensure sample's body lines appear in co_lines
body_linenos = {t[2] for t in lines_list if t[2] is not None}
print("co_lines covers body:", co.co_firstlineno + 1 in body_linenos)

pos_it = co.co_positions()
print("co_positions is iterator:", iter(pos_it) is pos_it)
pos_list = list(pos_it)
print("co_positions non-empty:", len(pos_list) > 0)
print(
    "co_positions entry shapes:",
    all(
        isinstance(t, tuple)
        and len(t) == 4
        and all(x is None or isinstance(x, int) for x in t)
        for t in pos_list
    ),
)

print("co_linetable is bytes:", isinstance(co.co_linetable, bytes))
with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    print("co_lnotab is bytes:", isinstance(co.co_lnotab, bytes))
print("co_exceptiontable is bytes:", isinstance(co.co_exceptiontable, bytes))

# code.replace(**changes)
co2 = co.replace(co_name="renamed_sample", co_filename="custom_file.py", co_firstlineno=500)
print("replace new attrs:", co2.co_name, co2.co_filename, co2.co_firstlineno)
print("replace preserves orig:", co.co_name, co.co_firstlineno != 500)
print("replace preserves varnames:", co2.co_varnames == co.co_varnames, co2.co_argcount == co.co_argcount)

# replace() on compile()-produced code object and unified CodeType
compiled = compile("ans = 40 + 2", "orig.py", "exec")
print("unified CodeType:", type(compiled) is type(co))
compiled2 = compiled.replace(co_filename="replaced.py")
print("compiled replace filename:", compiled2.co_filename, compiled.co_filename)
ns_r = {}
exec(compiled2, ns_r)
print("exec replaced code:", ns_r["ans"])

# replace() error validation
try:
    co.replace("positional")
except TypeError as e:
    print("replace pos err:", type(e).__name__)
try:
    co.replace(not_a_field=1)
except TypeError as e:
    print("replace bad kw err:", type(e).__name__)

# 5. sys.stdlib_module_names
print("stdlib_module_names is frozenset:", isinstance(sys.stdlib_module_names, frozenset))
print(
    "stdlib_module_names key entries:",
    all(
        name in sys.stdlib_module_names
        for name in ("opcode", "_opcode", "ast", "sys", "antigravity", "this", "zoneinfo", "builtins")
    ),
)
if sys.version_info[:2] == (3, 12):
    print("stdlib_module_names count 3.12:", len(sys.stdlib_module_names))
