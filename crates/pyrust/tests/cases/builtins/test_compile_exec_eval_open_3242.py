# CPython 3.12 parity tests for issue #3242:
# compile(ast/bytes/PathLike/flags/optimize/dont_inherit),
# exec(bytes/closure=), eval(bytes), open(opener=), __build_class__, WindowsError.

import ast
import builtins
import os
import pathlib
import sys
import tempfile
import types

# 1. compile() with ast.AST objects
mod_node = ast.parse("result = 6 * 7")
code_mod = compile(mod_node, "<ast-mod>", "exec")
ns = {}
exec(code_mod, ns)
print("compile ast.Module:", ns["result"], code_mod.co_filename)

expr_node = ast.parse("10 + 20 * 3", mode="eval")
code_expr = compile(expr_node, "<ast-expr>", "eval")
print("compile ast.Expression:", eval(code_expr), code_expr.co_filename)

inter_node = ast.parse("val = 99", mode="single")
code_inter = compile(inter_node, "<ast-single>", "single")
ns2 = {}
exec(code_inter, ns2)
print("compile ast.Interactive:", ns2["val"])

# Mismatched AST node type for mode
try:
    compile(expr_node, "<bad>", "exec")
except TypeError as e:
    print("ast mode mismatch:", type(e).__name__, "Module" in str(e))

# 2. compile() with bytes / bytearray (UTF-8 BOM and PEP 263 coding cookie)
bom_src = b"\xef\xbb\xbfbom_val = 123\n"
ns_bom = {}
exec(compile(bom_src, "<bom>", "exec"), ns_bom)
print("compile bom bytes:", ns_bom["bom_val"])

latin1_src = b"# -*- coding: latin-1 -*-\ns = '\xe9'\n"
ns_lat = {}
exec(compile(latin1_src, "<latin1>", "exec"), ns_lat)
print("compile latin1:", ord(ns_lat["s"]))

ba_src = bytearray(b"3 + 4 * 5")
print("compile bytearray eval:", eval(compile(ba_src, "<ba>", "eval")))

# 3. compile() with os.PathLike and bytes filename
p = pathlib.Path("virtual_dir/sample.py")
co_path = compile("x = 1", p, "exec")
print("compile PathLike filename:", co_path.co_filename == str(p))

co_bname = compile("x = 1", b"bytes_name.py", "exec")
print("compile bytes filename:", co_bname.co_filename)

# 4. compile() with PyCF_ONLY_AST, PyCF_TYPE_COMMENTS, PyCF_ALLOW_TOP_LEVEL_AWAIT, mode="func_type"
parsed_mod = compile("a = 1  # type: int\n", "<tc>", "exec", ast.PyCF_ONLY_AST | ast.PyCF_TYPE_COMMENTS)
print("PyCF_ONLY_AST Module:", isinstance(parsed_mod, ast.Module), parsed_mod.body[0].type_comment)

parsed_ft = compile("(int, str) -> bool", "<ft>", "func_type", ast.PyCF_ONLY_AST)
print("func_type AST:", isinstance(parsed_ft, ast.FunctionType), len(parsed_ft.argtypes))

try:
    compile("(int) -> str", "<ft>", "func_type")
except ValueError as e:
    print("func_type without flag:", type(e).__name__, "PyCF_ONLY_AST" in str(e))

co_tla = compile("await coro()", "<tla>", "exec", ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
print("PyCF_ALLOW_TOP_LEVEL_AWAIT flag:", bool(co_tla.co_flags & 0x80))

# Passing an AST node with PyCF_ONLY_AST returns an ast.Module node
same_node = compile(mod_node, "<ast>", "exec", ast.PyCF_ONLY_AST)
print("AST + PyCF_ONLY_AST:", isinstance(same_node, ast.Module))

# 5. compile() optimize=0, 1, 2 and invalid optimize
opt_src = '''"module docstring"
def f():
    "func docstring"
    assert False, "should fail at opt=0"
    return __debug__
class C:
    "class docstring"
    pass
'''
for level in (0, 1, 2):
    co_opt = compile(opt_src, "<opt>", "exec", optimize=level)
    ns_opt = {}
    exec(co_opt, ns_opt)
    try:
        dbg = ns_opt["f"]()
    except AssertionError:
        dbg = "AssertionError"
    print(
        f"optimize={level}:",
        ns_opt.get("__doc__"),
        ns_opt["f"].__doc__,
        ns_opt["C"].__doc__,
        dbg,
    )

for bad_opt in (-2, 3):
    try:
        compile("x = 1", "<t>", "exec", optimize=bad_opt)
    except ValueError as e:
        print(f"bad optimize={bad_opt}:", type(e).__name__)

# 6. compile() dont_inherit with __future__.annotations
future_test_src = '''from __future__ import annotations
c_inherit = compile("def g(x: NonExistent) -> AlsoMissing: pass", "<inh>", "exec", dont_inherit=False)
c_no_inherit = compile("def g(x: int) -> str: pass", "<noinh>", "exec", dont_inherit=True)
ns1, ns2 = {}, {}
exec(c_inherit, ns1)
exec(c_no_inherit, ns2)
out = (ns1["g"].__annotations__, ns2["g"].__annotations__)
'''
ns_fut = {}
exec(future_test_src, ns_fut)
print("dont_inherit=False:", ns_fut["out"][0])
print("dont_inherit=True:", ns_fut["out"][1]["x"] is int, ns_fut["out"][1]["return"] is str)

# 7. exec() and eval() with bytes / bytearray
ns_bytes = {}
exec(b"bx = 11 + 22", ns_bytes)
exec(bytearray(b"by = bx * 2"), ns_bytes)
print("exec bytes/bytearray:", ns_bytes["bx"], ns_bytes["by"])
print("eval bytes/bytearray:", eval(b"2 ** 8"), eval(bytearray(b"3 ** 4")))

# 8. exec(..., closure=...) Python 3.11+ keyword-only argument
def make_closure_code():
    x = 10
    y = 20
    def inner():
        nonlocal x
        x = x + y
        global captured_out
        captured_out = x
    return inner.__code__

closure_code = make_closure_code()
c1 = types.CellType(100)
c2 = types.CellType(23)
ns_cl = {}
exec(closure_code, ns_cl, closure=(c1, c2))
print("exec closure result:", ns_cl["captured_out"], c1.cell_contents, c2.cell_contents)

# Exact CPython 3.12 closure validation errors
for label, fn in [
    ("str+closure", lambda: exec("a = 1", {}, closure=())),
    ("no-free+closure", lambda: exec(compile("a = 1", "<t>", "exec"), {}, closure=())),
    ("missing-closure", lambda: exec(closure_code, {})),
    ("wrong-type", lambda: exec(closure_code, {}, closure=123)),
    ("wrong-len", lambda: exec(closure_code, {}, closure=(types.CellType(1),))),
    ("non-cell", lambda: exec(closure_code, {}, closure=(types.CellType(1), 99))),
]:
    try:
        fn()
    except TypeError as e:
        print(f"exec {label}:", type(e).__name__, str(e))

# 9. open(..., opener=...)
opener_calls = []
with tempfile.TemporaryDirectory() as tmpdir:
    target = os.path.join(tmpdir, "opener_test.txt")

    def custom_opener(path, flags):
        opener_calls.append((os.path.basename(path), isinstance(flags, int)))
        return os.open(path, flags)

    with open(target, "w", opener=custom_opener) as f:
        f.write("hello opener")
    with open(target, "r", opener=custom_opener) as f:
        content = f.read()
    print("open opener:", content, opener_calls)

    try:
        open(target, "r", opener=lambda p, fl: -1)
    except ValueError as e:
        print("opener -1:", type(e).__name__)
    try:
        open(target, "r", opener=lambda p, fl: "not-an-fd")
    except TypeError as e:
        print("opener non-int:", type(e).__name__)

# 10. __build_class__ on builtins
print("__build_class__ in builtins:", hasattr(builtins, "__build_class__"), __build_class__ is builtins.__build_class__)

class Base:
    base_attr = 7

class TagMeta(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kw):
        return {"tag": kw.get("tag", "none")}
    def __new__(mcls, name, bases, ns, **kw):
        return super().__new__(mcls, name, bases, ns)

def empty_body():
    pass

Built = __build_class__(empty_body, "Built", Base, metaclass=TagMeta, tag="custom")
print("__build_class__ result:", Built.__name__, Built.base_attr, Built.tag, issubclass(Built, Base))

for label, fn in [
    ("not enough args", lambda: __build_class__(empty_body)),
    ("non-func", lambda: __build_class__(123, "X")),
    ("non-str", lambda: __build_class__(empty_body, 123)),
]:
    try:
        fn()
    except TypeError as e:
        print(f"__build_class__ {label}:", type(e).__name__, str(e))

# 11. WindowsError on Windows (alias of OSError)
if sys.platform == "win32":
    print("WindowsError:", WindowsError is OSError, builtins.WindowsError is OSError)
else:
    print("WindowsError absent on non-windows:", not hasattr(builtins, "WindowsError"))
