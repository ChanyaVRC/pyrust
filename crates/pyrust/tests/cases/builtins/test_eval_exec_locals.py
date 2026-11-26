# Issue #3317: argument-omitted eval()/exec() use the calling frame's globals
# and locals (fast locals plus cell/free closure variables), CPython 3.12.3
# builtin_eval_impl / builtin_exec_impl semantics:
#   - globals=None, locals=None -> caller globals + caller locals mapping
#   - globals=G, locals=None    -> G is both globals and locals
#   - globals=None, locals=L    -> caller globals + L
# Covers module scope, class bodies, functions, closures, PEP 709 inlined
# comprehensions, generators, coroutines, and eval walrus targets.

module_value = "module-global"


def show(label, fn):
    try:
        print(label, repr(fn()))
    except Exception as exc:
        print(label, "EXC", type(exc).__name__, exc)


def user_names(mapping):
    return sorted(k for k in mapping if not k.startswith("__"))


print("--- issue reproducer ---")


def f():
    a = 1
    return eval("a + 1")


def g():
    x = 5
    return eval("[x for _ in range(2)]")


def outer():
    v = 10

    def inner():
        return eval("v + 5")

    return inner()


def outer_captured():
    v = 10

    def inner():
        v  # noqa: B018 - makes v a free variable of inner
        return eval("v + 5")

    return inner()


def h():
    y = 3
    exec("z = y * 2")
    return locals().get("z")


show("f", f)
show("g", g)
# CPython: `v` is not a free variable of inner, so eval cannot see it.
show("outer (not captured)", outer)
show("outer (captured)", outer_captured)
show("h", h)

print("--- module scope ---")
mod_x = 7
show("eval", lambda: eval("mod_x + 1"))
exec("mod_new = mod_x * 3")
print("exec", mod_new)
mod_locals = {"mod_x": 100}
print("eval None,locals", eval("mod_x + len(module_value)", None, mod_locals))
exec("mod_assigned = mod_x", None, mod_locals)
print("exec None,locals", mod_locals.get("mod_assigned"), "mod_assigned" in globals())
custom = {"mod_x": 55}
print("eval custom globals", eval("mod_x", custom), user_names(custom))
exec("custom_new = mod_x + 1", custom)
print("exec custom globals", custom["custom_new"], "custom_new" in globals())
print("comprehension", [eval("mod_x + i") for i in range(3)])
print("comprehension leaks i", "i" in globals())

print("--- functions ---")


def f_global():
    return eval("module_value")


def f_shadow():
    module_value = "local-shadow"
    return eval("module_value")


def f_args(p, q=2, *rest, kw=3, **kws):
    return eval("(p, q, rest, kw, kws)")


def f_unbound():
    return eval("later")
    later = 1  # noqa: F841 - makes `later` a local


def f_deleted():
    a = 1
    del a
    return eval("a")


def f_rebind():
    a = 1
    first = eval("a")
    a = 2
    return first, eval("a")


def f_builtin_shadow():
    len = lambda value: 42  # noqa: E731
    return eval("len([1])")


def f_compiled_eval():
    a = 9
    return eval(compile("a * 2", "<code>", "eval"))


def f_compiled_exec_via_eval():
    a = 9
    result = eval(compile("res = a * 2", "<code>", "exec"))
    return result, locals().get("res")


def f_explicit_none():
    a = 1
    return eval("a", None, None)


def f_genexp_inside_eval():
    x = 5
    return eval("list(x for _ in range(2))")


def f_lambda_inside_eval():
    x = 5
    return eval("(lambda: x)()")


def f_recursion(n):
    if n == 0:
        return eval("n")
    return eval("f_recursion(n - 1) + n")


show("global", f_global)
show("shadow", f_shadow)
show("args", lambda: f_args(1, 4, 5, kw=6, z=7))
show("unbound", f_unbound)
show("deleted", f_deleted)
show("rebind", f_rebind)
show("builtin shadow", f_builtin_shadow)
show("compiled eval", f_compiled_eval)
show("compiled exec via eval", f_compiled_exec_via_eval)
show("explicit None, None", f_explicit_none)
# Nested scopes inside eval'd code do not capture the caller's locals.
show("genexp inside eval", f_genexp_inside_eval)
show("lambda inside eval", f_lambda_inside_eval)
show("recursion", lambda: f_recursion(3))

print("--- explicit namespaces inside functions ---")


def f_custom_globals():
    a = 1
    namespace = {"b": 2}
    try:
        return eval("a", namespace)
    except NameError as exc:
        return "NameError", str(exc), eval("b", namespace)


def f_custom_globals_exec():
    namespace = {"b": 2}
    exec("c = b + 1", namespace)
    return namespace["c"], "c" in locals()


def f_globals_both():
    a = 1
    namespace = {"a": 99}
    return eval("a", namespace), eval("a", namespace, None), eval("a", None, None)


def f_none_locals():
    a = 1
    mapping = {"a": 50}
    return eval("(a, module_value)", None, mapping)


def f_none_locals_exec():
    a = 1
    mapping = {"a": 50}
    exec("b = a + len(module_value)", None, mapping)
    return mapping["b"], a


show("custom globals", f_custom_globals)
show("custom globals exec", f_custom_globals_exec)
show("globals as locals", f_globals_both)
show("None, locals", f_none_locals)
show("None, locals exec", f_none_locals_exec)

print("--- locals mapping identity and writes ---")


def f_exec_then_eval():
    y = 3
    exec("z = y * 2")
    return eval("z")


def f_exec_no_fast_write():
    a = 1
    exec("a = 2")
    return a, locals()["a"]


def f_locals_identity():
    a = 1
    before = locals()
    return eval("locals()") is before, eval("globals()") is globals()


def f_eval_locals_names():
    a = 1
    return user_names(eval("locals()"))


def f_exec_def_uses_local():
    a = 7
    exec("def nested(): return a")
    try:
        return locals()["nested"]()
    except NameError:
        return "NameError"


show("exec then eval", f_exec_then_eval)
show("exec no fast write", f_exec_no_fast_write)
show("locals identity", f_locals_identity)
show("eval locals()", f_eval_locals_names)
show("exec def uses local", f_exec_def_uses_local)

print("--- closures ---")


def closure_cellvar():
    v = 10

    def inner():
        return v

    return eval("v + 1"), inner()


def closure_nonlocal():
    v = 10

    def inner():
        nonlocal v
        v += 1
        return eval("v")

    return inner(), v


def closure_deep():
    v = 1

    def mid():
        w = 2

        def inner():
            v, w  # noqa: B018 - capture both
            return eval("v + w")

        return inner()

    return mid()


def closure_exec_cell():
    v = 10

    def inner():
        return v

    exec("r = v * 2")
    return locals().get("r"), inner()


def closure_lambda():
    x = 3  # noqa: F841 - not captured by the lambda
    return (lambda: eval("x"))()


def closure_lambda_captured():
    x = 3
    return (lambda: (x, eval("x"))[1])()


show("cellvar", closure_cellvar)
show("nonlocal", closure_nonlocal)
show("deep", closure_deep)
show("exec cell", closure_exec_cell)
show("lambda", closure_lambda)
show("lambda captured", closure_lambda_captured)

print("--- comprehensions (PEP 709) ---")


def comp_eval():
    base = 100
    return [eval("base + i") for i in range(3)]


def comp_dict_set():
    base = 1
    return {k: eval("k + base") for k in range(2)}, sorted({eval("k * base") for k in range(3)})


def comp_nested():
    a = 10
    return [[eval("a + i + j") for j in range(2)] for i in range(2)]


def comp_shadow():
    i = "outer"
    result = [eval("i") for i in range(2)]
    return result, i, eval("i"), locals()["i"]


def comp_locals():
    a = 1
    inside = [user_names(locals()) for i in range(1)]
    return inside, user_names(vars()), user_names(locals())


def comp_exec_assign():
    a = 1
    [exec("q = a + i") for i in range(2)]
    return user_names(locals()), locals().get("q")


def genexp_eval():
    base = 100
    return list(eval("i") for i in range(3))


def genexp_eval_uncaptured():
    base = 100  # noqa: F841 - not visible inside the generator expression
    return list(eval("base") for i in range(3))


def genexp_eval_captured():
    base = 100
    return list(eval("base") for i in range(3) if base)


def eval_dict_comprehension():
    return eval("[x for _ in range(2)]", {}, {"x": 5}), eval(
        "[[x + y for y in range(2)] for _ in range(2)]", {}, {"x": 5}
    )


def exec_dict_comprehension():
    mapping = {"x": 5}
    exec("r = [x for _ in range(2)]", {}, mapping)
    return mapping["r"]


show("eval in comp", comp_eval)
show("dict/set comp", comp_dict_set)
show("nested comp", comp_nested)
show("shadowed iteration var", comp_shadow)
show("locals in comp", comp_locals)
show("exec in comp", comp_exec_assign)
show("genexp", genexp_eval)
show("genexp uncaptured", genexp_eval_uncaptured)
show("genexp captured", genexp_eval_captured)
show("eval explicit locals comp", eval_dict_comprehension)
show("exec explicit locals comp", exec_dict_comprehension)

print("--- generators and coroutines ---")


def gen():
    a = 1
    yield eval("a")
    a = 2
    yield eval("a")
    exec("b = a + 1")
    yield locals().get("b")
    yield [eval("a + i") for i in range(2)]


async def coro():
    a = 5
    return eval("a"), [eval("a + i") for i in range(2)]


def run_coro():
    c = coro()
    try:
        c.send(None)
    except StopIteration as stop:
        return stop.value


async def agen():
    a = 3
    yield eval("a")
    yield [eval("a + i") for i in range(2)]


def run_agen():
    it = agen()
    out = []
    while True:
        try:
            it.asend(None).send(None)
        except StopIteration as stop:
            out.append(stop.value)
        except StopAsyncIteration:
            return out


show("generator", lambda: list(gen()))
show("coroutine", run_coro)
show("async generator", run_agen)

print("--- class bodies ---")


class C:
    cx = 1
    cy = eval("cx + 1")
    exec("cz = cx + cy")
    after_exec = cz + 1  # noqa: F821 - bound by exec above
    exec("cx = 10")
    seen_cx = cx
    cw = eval("module_value")
    comp = [eval("cx * i") for i in range(3)]
    dict_comp = {i: eval("i") for i in range(2)}


print("class", C.cy, C.cz, C.after_exec, C.cx, C.seen_cx, C.cw, C.comp, C.dict_comp)


class D:
    dv = 1
    try:
        plain_comp = [dv for i in range(2)]  # noqa: F821 - class scope is skipped
    except NameError as exc:
        plain_comp = str(exc)


print("class plain comp", D.plain_comp)


def class_in_function():
    fv = 3

    class E:
        ex = 1
        ey = eval("ex + 1")
        try:
            ez = eval("fv")
        except NameError:
            ez = "NameError"

    return E.ey, E.ez


def class_in_function_captured():
    fv = 3

    class E:
        ex = fv
        ez = eval("fv")

    return E.ez


class Methods:
    mx = 1

    def class_var(self):
        return eval("mx")

    def dunder_class(self):
        return eval("__class__")

    def dunder_class_captured(self):
        __class__  # noqa: B018 - creates the implicit closure cell
        return eval("__class__").__name__

    def self_arg(self):
        return type(eval("self")).__name__


show("class in function", class_in_function)
show("class in function captured", class_in_function_captured)
show("method class var", lambda: Methods().class_var())
show("method __class__", lambda: Methods().dunder_class())
show("method __class__ captured", lambda: Methods().dunder_class_captured())
show("method self", lambda: Methods().self_arg())

print("--- eval walrus targets ---")


def walrus_in_function():
    a = 4
    result = eval("(w := a + 1)")
    return result, user_names(locals()), "w" in globals()


def walrus_compiled():
    a = 1
    result = eval(compile("(b := a + 1)", "<code>", "eval"))
    return result, user_names(locals()), "b" in globals()


def walrus_explicit():
    g_ns, l_ns = {}, {}
    first = eval("(w := 3)", g_ns, l_ns)
    g2 = {}
    second = eval("(w := 3)", g2)
    l3 = {"w": 1}
    third = eval("(w, (w := w + 1), w)", {}, l3)
    g4, l4 = {"len": 3}, {}
    fourth = eval("(len, (len := 4), len)", g4, l4)
    return (
        (first, user_names(g_ns), l_ns),
        (second, user_names(g2)),
        (third, l3),
        (fourth, g4["len"], l4),
    )


def walrus_unbound():
    try:
        return eval("(w, (w := 1))", {}, {})
    except NameError as exc:
        return "NameError", str(exc)


def walrus_comprehension():
    g_ns, l_ns = {}, {}
    result = eval("[(w := i) for i in range(3)]", g_ns, l_ns)
    return result, user_names(g_ns), l_ns


show("walrus in function", walrus_in_function)
show("walrus compiled", walrus_compiled)
show("walrus explicit", walrus_explicit)
show("walrus unbound", walrus_unbound)
show("walrus comprehension", walrus_comprehension)
mod_walrus = 0
print("module walrus", eval("(mod_walrus := 4)"), mod_walrus)
