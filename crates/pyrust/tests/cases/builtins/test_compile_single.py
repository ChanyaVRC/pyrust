import builtins
import io
import sys


def show(label, fn):
    try:
        res = fn()
        print(f"{label}: OK {res!r}")
    except BaseException as exc:
        print(f"{label}: EXC {type(exc).__name__}")


# --- default sys.displayhook + redirected sys.stdout + builtins._ ---
orig_stdout = sys.stdout
orig_dh = sys.displayhook
had_underscore = hasattr(builtins, "_")
orig_underscore = getattr(builtins, "_", None)

try:
    buf = io.StringIO()
    sys.stdout = buf
    exec(compile("1 + 2", "<single_default>", "single"), {})
    print_1 = buf.getvalue()
    u1 = builtins._

    # None is suppressed by default displayhook and does not overwrite builtins._
    buf2 = io.StringIO()
    sys.stdout = buf2
    exec(compile("None", "<single_none>", "single"), {})
    print_none = buf2.getvalue()
    u2 = builtins._

    # String literal is displayed (NOT treated as module __doc__)
    buf3 = io.StringIO()
    sys.stdout = buf3
    ns_doc = {}
    exec(compile('"hello"', "<single_str>", "single"), ns_doc)
    print_str = buf3.getvalue()
    has_doc = "__doc__" in ns_doc

    # Custom __repr__ is respected by default displayhook
    class Fancy:
        def __repr__(self):
            return "Fancy<custom>"

    buf4 = io.StringIO()
    sys.stdout = buf4
    exec(compile("Fancy()", "<single_repr>", "single"), {"Fancy": Fancy})
    print_fancy = buf4.getvalue()
finally:
    sys.stdout = orig_stdout
    if had_underscore:
        builtins._ = orig_underscore
    elif hasattr(builtins, "_"):
        delattr(builtins, "_")

print("default expr stdout:", repr(print_1), "_:", u1)
print("default None stdout:", repr(print_none), "_:", u2)
print("default str stdout:", repr(print_str), "has __doc__:", has_doc)
print("default custom repr stdout:", repr(print_fancy))


# --- custom sys.displayhook captures every expression statement (including None) ---
seen = []


def record(val):
    seen.append(val)


try:
    sys.displayhook = record

    def run_single(label, src, ns=None):
        seen.clear()
        g = {} if ns is None else ns
        co = compile(src, "<single_test>", "single")
        ret = exec(co, g)
        print(f"{label}: ret={ret!r} seen={seen!r}")
        return g, co

    run_single("expr", "2 * 21")
    run_single("expr None", "None")
    run_single("str literal", '"not a docstring"')
    g_assign, _ = run_single("assign", "x = 10")
    print("assign x:", g_assign["x"])
    g_aug, _ = run_single("augassign", "x += 5", {"x": 10})
    print("augassign x:", g_aug["x"])
    g_walrus, _ = run_single("walrus", "(w := 7)")
    print("walrus w:", g_walrus["w"])
    run_single("pass", "pass")
    run_single("semicolon exprs", "1; 2")
    run_single("semicolon trailing", "9;")
    g_semi, _ = run_single("semicolon assign+expr", "a = 3; a * 4")
    print("semi a:", g_semi["a"])

    # Leading/trailing blank lines & comments, multiline expr
    run_single("leading nl+comment", "\n# header\n10 + 5")
    run_single("trailing nl+comment", "x = 1\n\n# tail\n")
    run_single("multiline expr", "(\n1 +\n2 +\n3\n)")

    # Compound statements: top-level nested expressions invoke displayhook
    run_single("if expr", "if True:\n    1 + 2\n")
    run_single("if None", "if True:\n    None\n")
    run_single("if inline semi", "if True: 1; 2\n")
    run_single("nested if", "if True:\n    if True:\n        55\n")
    run_single("for expr", "for i in [1, 2, 3]:\n    i * 10\n")
    run_single("while expr", "while n > 0:\n    n\n    n -= 1\n", {"n": 2})
    run_single("try finally expr", "try:\n    42\nfinally:\n    99\n")

    class Ctx:
        def __enter__(self):
            return 11

        def __exit__(self, *a):
            return False

    run_single("with expr", "with Ctx() as v:\n    v + 1\n", {"Ctx": Ctx})
    run_single("match expr", "match 2:\n    case 2:\n        77\n")

    # Expressions inside def and class bodies do NOT invoke displayhook
    g_def, _ = run_single("def body", "def f():\n    1 + 2\n    return 3\n")
    seen.clear()
    print("call defined f:", g_def["f"](), "seen:", seen)

    g_cls, _ = run_single("class body", "class C:\n    1 + 2\n    v = 9\n")
    print("class C.v:", g_cls["C"].v)

    # Explicit globals + locals
    seen.clear()
    g_ns = {"base": 100}
    l_ns = {}
    exec(compile("out = base + 23; out", "<single_gl>", "single"), g_ns, l_ns)
    print("gl locals out:", l_ns["out"], "in globals:", "out" in g_ns, "seen:", seen)

    # eval() with single-mode code object
    seen.clear()
    ns_ev = {}
    ev_ret = eval(compile("10 + 20", "<eval_single>", "single"), ns_ev)
    print("eval single expr:", ev_ret, "seen:", seen)
    seen.clear()
    ev_assign = eval(compile("z = 5", "<eval_single_assign>", "single"), ns_ev)
    print("eval single assign:", ev_assign, "z:", ns_ev.get("z"), "seen:", seen)

    # Deleted sys.displayhook raises RuntimeError
    del sys.displayhook
    try:
        exec(compile("1", "<del_dh>", "single"), {})
    except RuntimeError as exc:
        print("deleted displayhook expr:", exc.args)
    try:
        exec(compile("None", "<del_dh_none>", "single"), {})
    except RuntimeError as exc:
        print("deleted displayhook None:", exc.args)
finally:
    sys.displayhook = orig_dh


# --- code object attributes & traceback filename ---
co = compile("1 + 2", "custom_single.py", "single")
print("code type:", type(co).__name__)
print("co_filename:", co.co_filename)
print("co_name:", co.co_name)
print("co_qualname:", co.co_qualname)
print("co_firstlineno:", co.co_firstlineno)
print("co_argcount:", co.co_argcount)
print("co_posonlyargcount:", co.co_posonlyargcount)
print("co_kwonlyargcount:", co.co_kwonlyargcount)
print("co_nlocals:", co.co_nlocals)
print("co_varnames:", co.co_varnames)
print("co_freevars:", co.co_freevars)
print("co_cellvars:", co.co_cellvars)

try:
    exec(compile("1 / 0", "custom_single_err.py", "single"), {})
except ZeroDivisionError as exc:
    tb = exc.__traceback__
    while tb.tb_next is not None:
        tb = tb.tb_next
    print("tb filename:", tb.tb_frame.f_code.co_filename)


# --- SyntaxError cases in 'single' mode ---
for label, src in [
    ("empty", ""),
    ("newline", "\n"),
    ("spaces+nl", "   \n"),
    ("comment", "# hi"),
    ("comment+nl", "# hi\n"),
    ("two expr lines", "1\n2"),
    ("two assign lines", "x = 1\ny = 2"),
    ("semi + second line", "1; 2\n3"),
    ("compound + trailing stmt", "if True:\n    1\nx = 2\n"),
    ("two defs", "def f(): pass\ndef g(): pass"),
    ("inline if no nl", "if True: 1"),
    ("inline def no nl", "def f(): return 1"),
]:
    show(f"syntax {label}", lambda s=src: compile(s, "<bad>", "single"))
