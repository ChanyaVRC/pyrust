# CPython 3.12 parity tests for `contextvars` (PEP 567, issue #3184).

import asyncio
import collections.abc
import contextvars
import decimal

# --- Module surface ---
# Note: `collections.abc.Mapping.register(Context)` was added in CPython 3.13;
# `Context` implements the Mapping interface (`Collection` structural check is
# True in both CPython 3.12 and 3.13+).
print("all:", contextvars.__all__)
print("ctx_is_collection_cls:", issubclass(contextvars.Context, collections.abc.Collection))
print("ctx_is_collection_inst:", isinstance(contextvars.Context(), collections.abc.Collection))
print("token_missing_repr:", repr(contextvars.Token.MISSING))

# --- ContextVar creation & properties ---
v_nodef = contextvars.ContextVar("nodef")
v_def = contextvars.ContextVar("withdef", default=42)
v_none = contextvars.ContextVar("nonedef", default=None)
v_same_name = contextvars.ContextVar("withdef", default=99)

print("names:", v_nodef.name, v_def.name, v_none.name)
print("distinct_same_name:", v_def is not v_same_name, v_def == v_same_name)
print("repr_nodef:", repr(v_nodef).startswith("<ContextVar name='nodef' at 0x") and repr(v_nodef).endswith(">"))
print("repr_def:", repr(v_def).startswith("<ContextVar name='withdef' default=42 at 0x") and repr(v_def).endswith(">"))

# GenericAlias support
print("cv_generic:", contextvars.ContextVar[int].__args__)
print("tok_generic:", contextvars.Token[str].__args__)

# Invalid ContextVar creation
for bad_call in (
    lambda: contextvars.ContextVar(123),
    lambda: contextvars.ContextVar("x", 1),
    lambda: contextvars.ContextVar(),
):
    try:
        bad_call()
        print("cv_err_fail")
    except TypeError:
        print("cv_type_err_ok")

# Read-only .name
try:
    v_def.name = "changed"
    print("name_ro_fail")
except AttributeError:
    print("name_ro_ok")

# Direct Token() construction forbidden
try:
    contextvars.Token()
    print("tok_new_fail")
except RuntimeError:
    print("tok_new_ok")

# Direct Context(arg) forbidden
try:
    contextvars.Context(1)
    print("ctx_arg_fail")
except TypeError:
    print("ctx_arg_ok")

# --- get / set / reset ---
try:
    v_nodef.get()
    print("get_unset_fail")
except LookupError:
    print("get_unset_ok")

print("get_arg_default:", v_nodef.get("fallback"), v_nodef.get(None))
print("get_var_default:", v_def.get(), v_def.get("override"))
print("get_none_default:", v_none.get(), v_none.get("override"))

t1 = v_nodef.set("first")
print("after_set1:", v_nodef.get(), t1.var is v_nodef, t1.old_value is contextvars.Token.MISSING)
print("tok_repr_unused:", repr(t1).startswith("<Token var=<ContextVar name='nodef'") and "used" not in repr(t1))

t2 = v_nodef.set("second")
print("after_set2:", v_nodef.get(), t2.var is v_nodef, t2.old_value)

# Read-only token attributes
for attr in ("var", "old_value"):
    try:
        setattr(t2, attr, None)
        print("tok_ro_fail", attr)
    except AttributeError:
        print("tok_ro_ok", attr)

v_nodef.reset(t2)
print("after_reset2:", v_nodef.get())
print("tok_repr_used:", repr(t2).startswith("<Token used var=<ContextVar name='nodef'"))

v_nodef.reset(t1)
try:
    v_nodef.get()
    print("after_reset1_fail")
except LookupError:
    print("after_reset1_ok")

# --- Token.reset error cases ---
try:
    v_nodef.reset("not_a_token")
    print("reset_bad_type_fail")
except TypeError:
    print("reset_bad_type_ok")

try:
    v_nodef.reset(t1)
    print("reset_used_fail")
except RuntimeError:
    print("reset_used_ok")

t_other = v_def.set(100)
try:
    v_nodef.reset(t_other)
    print("reset_wrong_var_fail")
except ValueError:
    print("reset_wrong_var_ok")
v_def.reset(t_other)

# Reset in wrong context
c_a = contextvars.Context()
c_b = contextvars.Context()
t_in_a = c_a.run(v_def.set, 777)
try:
    c_b.run(v_def.reset, t_in_a)
    print("reset_wrong_ctx_fail")
except ValueError:
    print("reset_wrong_ctx_ok")
c_a.run(v_def.reset, t_in_a)
print("reset_in_a_ok:", v_def in c_a, c_a.get(v_def))

# --- Context Mapping interface ---
ctx = contextvars.Context()
print("empty_ctx:", len(ctx), list(ctx), v_def in ctx, ctx.get(v_def), ctx.get(v_def, "no"))
try:
    _ = ctx[v_def]
    print("ctx_missing_key_fail")
except KeyError:
    print("ctx_missing_key_ok")

# Non-ContextVar key checks
for op in (
    lambda: ctx["x"],
    lambda: "x" in ctx,
    lambda: ctx.get("x"),
):
    try:
        op()
        print("ctx_keytype_fail")
    except TypeError:
        print("ctx_keytype_ok")

ctx.run(lambda: (v_def.set(10), v_nodef.set("hi")))
print("ctx_len:", len(ctx))
print("ctx_contains:", v_def in ctx, v_nodef in ctx, v_none in ctx)
print("ctx_getitem:", ctx[v_def], ctx[v_nodef])
print("ctx_get:", ctx.get(v_def), ctx.get(v_none, "absent"))
print("ctx_iter_set:", len(list(ctx)), v_def in list(ctx), v_nodef in list(ctx))
print("ctx_keys:", len(ctx.keys()), v_def in ctx.keys())
print("ctx_values:", len(ctx.values()), 10 in ctx.values(), "hi" in ctx.values())
print("ctx_items:", len(ctx.items()), (v_def, 10) in ctx.items())

# Equality & hashing
ctx2 = ctx.copy()
print("ctx_eq_copy:", ctx == ctx2, ctx is not ctx2)
print("ctx_eq_dict:", ctx == {}, ctx == {v_def: 10, v_nodef: "hi"})
ctx2.run(v_def.set, 20)
print("ctx_eq_diff:", ctx == ctx2, ctx[v_def], ctx2[v_def])
try:
    hash(ctx)
    print("ctx_hash_fail")
except TypeError:
    print("ctx_hash_ok")

# --- Context.run: args, kwargs, nested contexts, exception restoration, re-entry ---
print("run_args_kwargs:", ctx.run(lambda a, b, c=3, callable=0: (a, b, c, callable), 1, 2, callable=9))

outer_var = contextvars.ContextVar("outer", default="init")
outer_var.set("main")

c1 = contextvars.Context()
c2 = contextvars.Context()

def nested_run():
    outer_var.set("in_c1")
    before = outer_var.get()
    def inner():
        inner_initial = outer_var.get()
        outer_var.set("in_c2")
        return inner_initial, outer_var.get()
    inner_res = c2.run(inner)
    after = outer_var.get()
    return before, inner_res, after

print("nested_run:", c1.run(nested_run), "main_after:", outer_var.get())

# Exception inside Context.run restores previous context and clears entered state
c_exc = contextvars.Context()
try:
    def boom():
        outer_var.set("in_exc")
        raise ZeroDivisionError("boom")
    c_exc.run(boom)
except ZeroDivisionError:
    pass
print("after_exc:", outer_var.get(), c_exc[outer_var])
# Can re-enter c_exc after exception exited it
print("reenter_after_exc:", c_exc.run(outer_var.get))

# Re-entering same Context while active raises RuntimeError
c_re = contextvars.Context()
try:
    c_re.run(lambda: c_re.run(lambda: None))
    print("reenter_fail")
except RuntimeError:
    print("reenter_ok")

# --- copy_context() ---
outer_var.set("before_copy")
snap = contextvars.copy_context()
outer_var.set("after_copy")
print("copy_context:", outer_var.get(), snap[outer_var], snap.run(outer_var.get))
snap.run(outer_var.set, "in_snap")
print("copy_isolated:", outer_var.get(), snap[outer_var])

# --- Integration with decimal ---
decimal.getcontext().prec = 28
d_ctx = contextvars.copy_context()
def change_decimal():
    decimal.setcontext(decimal.Context(prec=5))
    return str(decimal.Decimal("1") / decimal.Decimal("3"))
print("decimal_in_ctx:", d_ctx.run(change_decimal))
print("decimal_outside:", decimal.getcontext().prec)

# --- Integration with asyncio ---
async def test_async_contextvars():
    av = contextvars.ContextVar("av", default="default")
    av.set("parent")

    async def child(label):
        seen = av.get()
        await asyncio.sleep(0)
        av.set(label)
        return seen, av.get()

    t_a = asyncio.create_task(child("child_a"))
    t_b = asyncio.create_task(child("child_b"))
    res_a = await t_a
    res_b = await t_b
    print("async_tasks:", res_a, res_b, "parent:", av.get())

    explicit_ctx = contextvars.Context()
    explicit_ctx.run(av.set, "explicit_init")
    t_c = asyncio.create_task(child("explicit_done"), context=explicit_ctx)
    res_c = await t_c
    print("async_explicit_ctx:", res_c, "ctx_after:", explicit_ctx[av], "parent:", av.get(), "get_ctx:", t_c.get_context() is explicit_ctx)

asyncio.run(test_async_contextvars())

print("contextvars parity ok")
