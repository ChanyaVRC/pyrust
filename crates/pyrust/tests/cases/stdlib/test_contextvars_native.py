# CPython 3.12.3 parity for the native `contextvars` / `_contextvars` port
# (issue #3461).  Complements test_contextvars.py (the PEP 567 basics).
#
# `Context` iteration order in CPython follows its HAMT (hash order), so
# every multi-variable listing below is sorted by variable name.

import collections.abc
import contextvars
import copy
import pickle
import re
import threading

from contextvars import Context, ContextVar, Token, copy_context


def norm(text):
    return re.sub(r"0x[0-9a-fA-F]+", "0x?", str(text))


def attempt(label, fn):
    try:
        result = fn()
    except BaseException as exc:
        print(label, "->", type(exc).__name__ + ":", norm(exc))
    else:
        print(label, "=", norm(repr(result)))


def names(iterable):
    return sorted(var.name for var in iterable)


# --- module and class surface ---------------------------------------------------
print("all:", contextvars.__all__, type(contextvars.__all__).__name__)
print("module attrs:", [k for k in vars(contextvars) if not k.startswith("__") or k == "__all__"])
for cls in (ContextVar, Token, Context):
    print(cls.__name__, cls.__qualname__, cls.__module__, cls.__doc__, cls.__mro__ == (cls, object))
    print(" public:", [n for n in dir(cls) if not n.startswith("_") or n == "__class_getitem__"])
    attempt(" subclass", lambda cls=cls: type("Sub", (cls,), {}))
print("same objects:", contextvars.ContextVar is ContextVar, contextvars.Token is Token)
print("copy_context name:", copy_context.__name__)
print("Token.MISSING:", repr(Token.MISSING), type(Token.MISSING).__name__, type(Token.MISSING).__module__)
print("MISSING singleton:", Token.MISSING is Token.MISSING, bool(Token.MISSING))
print("mapping abc:", isinstance(Context(), collections.abc.Mapping), issubclass(Context, collections.abc.Mapping))
print("collection abc:", isinstance(Context(), collections.abc.Collection), isinstance(Context(), collections.abc.Iterable))

# --- ContextVar construction ----------------------------------------------------
attempt("CV()", lambda: ContextVar())
attempt("CV(1)", lambda: ContextVar(1))
attempt("CV(None)", lambda: ContextVar(None))
attempt("CV(a, 1)", lambda: ContextVar("a", 1))
attempt("CV(a, x=1)", lambda: ContextVar("a", x=1))
attempt("CV(name=a)", lambda: ContextVar(name="a"))
attempt("CV(a, name=a)", lambda: ContextVar("a", name="a"))
attempt("CV(a, 1, x=1)", lambda: ContextVar("a", 1, x=1))
attempt("CV(a, default=1, x=1)", lambda: ContextVar("a", default=1, x=2))
attempt("CV(default=1)", lambda: ContextVar(default=1))


class MyStr(str):
    pass


sub_name = ContextVar(MyStr("sub"))
print("str subclass name:", sub_name.name, type(sub_name.name).__name__)

plain = ContextVar("plain")
withdef = ContextVar("withdef", default=5)
nonedef = ContextVar("nonedef", default=None)
print("repr:", norm(plain), norm(withdef), norm(nonedef))
print("name:", plain.name, withdef.name)
attempt("set name", lambda: setattr(plain, "name", "x"))
attempt("del name", lambda: delattr(plain, "name"))
attempt("set attr", lambda: setattr(plain, "other", 1))
attempt("dict", lambda: plain.__dict__)
print("identity eq:", plain == plain, plain == ContextVar("plain"), plain != ContextVar("plain"))
print("hashable:", len({plain: 1, withdef: 2}), {plain: 1}[plain])
print("generic:", ContextVar[int], ContextVar[int].__origin__ is ContextVar, ContextVar[int, str].__args__)
print("generic token:", Token[str], Token[str].__args__)
attempt("Context[int]", lambda: Context[int].__args__)

# --- get / set / reset ----------------------------------------------------------
attempt("get unset", lambda: plain.get())
attempt("get arg", lambda: plain.get("fallback"))
attempt("get None arg", lambda: plain.get(None))
attempt("get default", lambda: withdef.get())
attempt("get arg beats default", lambda: withdef.get("explicit"))
attempt("get none default", lambda: nonedef.get())
attempt("get kw", lambda: plain.get(default=1))
attempt("get 2 args", lambda: plain.get(1, 2))
attempt("set none", lambda: plain.set())
attempt("set two", lambda: plain.set(1, 2))
attempt("set kw", lambda: plain.set(value=1))
attempt("reset none", lambda: plain.reset())
attempt("reset kw", lambda: plain.reset(token=1))
attempt("reset int", lambda: plain.reset(1))
attempt("reset str", lambda: plain.reset("abc"))
attempt("reset MISSING", lambda: plain.reset(Token.MISSING))

tok1 = plain.set("one")
print("tok1:", norm(tok1), tok1.var is plain, tok1.old_value is Token.MISSING)
tok2 = plain.set("two")
print("tok2:", tok2.old_value, plain.get())
tok_d = withdef.set(10)
print("tok_d old:", tok_d.old_value is Token.MISSING, withdef.get())
print("var is read-only:", end=" ")
for attr in ("var", "old_value"):
    attempt("set tok." + attr, lambda attr=attr: setattr(tok2, attr, 1))
    attempt("del tok." + attr, lambda attr=attr: delattr(tok2, attr))
attempt("tok attr", lambda: setattr(tok2, "extra", 1))
attempt("tok hash", lambda: hash(tok2))
print("tok eq:", tok2 == tok2, tok2 == tok1)
attempt("reset wrong var", lambda: plain.reset(tok_d))
print("reset ok:", plain.reset(tok2), plain.get())
print("tok2 used repr:", norm(tok2))
attempt("reset used", lambda: plain.reset(tok2))
attempt("reset used wrong var", lambda: withdef.reset(tok2))
print("reset tok1:", plain.reset(tok1))
attempt("get after reset", lambda: plain.get())
attempt("reset used again", lambda: plain.reset(tok1))
withdef.reset(tok_d)
print("withdef after reset:", withdef.get(), withdef in copy_context())

# Token reuse order: "used" is checked before the variable.
other = ContextVar("other")
t_other = other.set(1)
other.reset(t_other)
attempt("used token on wrong var", lambda: plain.reset(t_other))

attempt("Token()", lambda: Token())
attempt("Token(1)", lambda: Token(1))
attempt("Token.__new__", lambda: Token.__new__(Token))

# set/reset round trip leaves other variables alone
a = ContextVar("a")
b = ContextVar("b")
ta = a.set(1)
tb = b.set(2)
ta2 = a.set(3)
print("round trip:", a.get(), b.get(), ta2.old_value)
a.reset(ta2)
print("after reset ta2:", a.get(), b.get())
a.reset(ta)
b.reset(tb)
print("both unset:", a.get("none"), b.get("none"))

# reset in a different Context
ctx_x = Context()
ctx_y = Context()
tok_in_x = ctx_x.run(plain.set, "in x")
attempt("reset in other ctx", lambda: ctx_y.run(plain.reset, tok_in_x))
attempt("reset in main ctx", lambda: plain.reset(tok_in_x))
print("reset in own ctx:", ctx_x.run(plain.reset, tok_in_x), plain in ctx_x, len(ctx_x))
attempt("repr after other ctx", lambda: tok_in_x)

# --- pickling / copying / weak references --------------------------------------
for label, obj in (("var", plain), ("ctx", Context()), ("tok", plain.set("p"))):
    attempt("pickle " + label, lambda obj=obj: pickle.dumps(obj))
    attempt("copy " + label, lambda obj=obj: copy.copy(obj))
    attempt("reduce_ex " + label, lambda obj=obj: obj.__reduce_ex__(2))
attempt("weakref var", lambda: __import__("weakref").ref(plain))
attempt("weakref tok", lambda: __import__("weakref").ref(tok1))
print("weakref ctx:", __import__("weakref").ref(ctx_x)() is ctx_x)

# --- Context mapping protocol ---------------------------------------------------
c = Context()
print("empty:", len(c), bool(c), list(c), c == Context(), dict(c))
attempt("Context(1)", lambda: Context(1))
attempt("Context(a=1)", lambda: Context(a=1))
attempt("Context repr", lambda: c)
attempt("Context hash", lambda: hash(c))
attempt("c[1]", lambda: c[1])
attempt("1 in c", lambda: 1 in c)
attempt("c.get(1)", lambda: c.get(1))
attempt("c.get()", lambda: c.get())
attempt("c.get(v,1,2)", lambda: c.get(plain, 1, 2))
attempt("c.get kw", lambda: c.get(plain, default=1))
attempt("c[var]", lambda: c[plain])
attempt("c.get(var)", lambda: c.get(plain))
attempt("c.get(var, d)", lambda: c.get(plain, "d"))
try:
    c[plain]
except KeyError as exc:
    print("KeyError arg is var:", exc.args[0] is plain)

c.run(lambda: (a.set("A"), b.set("B")))
print("len:", len(c), bool(c))
print("contains:", a in c, b in c, plain in c)
print("getitem:", c[a], c[b])
print("get:", c.get(a), c.get(plain), c.get(plain, "dflt"))
print("keys:", names(c.keys()), names(c), names(iter(c)))
print("values:", sorted(c.values()))
print("items:", sorted((k.name, v) for k, v in c.items()))
print("view types:", type(c.keys()).__name__, type(c.values()).__name__, type(c.items()).__name__, type(iter(c)).__name__)
print("view module:", type(c.keys()).__module__, type(c.values()).__module__)
print("view lens:", len(c.keys()), len(c.values()), len(c.items()))
it = c.keys()
print("view is iterator:", iter(it) is it, c.keys() is not c.keys())
first = next(it)
second = next(it)
attempt("view exhausted", lambda: next(it))
print("view yielded:", sorted([first.name, second.name]), len(it))
attempt("keys(1)", lambda: c.keys(1))
attempt("keys(x=1)", lambda: c.keys(x=1))
attempt("values(1)", lambda: c.values(1))
attempt("items(1)", lambda: c.items(1))
attempt("copy(1)", lambda: c.copy(1))
attempt("copy(x=1)", lambda: c.copy(x=1))
attempt("copy_context(1)", lambda: copy_context(1))
attempt("copy_context(x=1)", lambda: copy_context(x=1))
attempt("unbound copy()", lambda: Context.copy())
attempt("unbound copy(1)", lambda: Context.copy(1))
attempt("unbound get(1)", lambda: ContextVar.get(1))
attempt("len(1 arg)", lambda: c.__len__(1))
attempt("getitem()", lambda: c.__getitem__())
attempt("getitem(1,2)", lambda: c.__getitem__(1, 2))
attempt("contains()", lambda: c.__contains__())
attempt("next(1 arg)", lambda: iter(c).__next__(1))

# snapshot semantics: views do not see later changes
snap_keys = c.keys()
c.run(lambda: ContextVar("late").set(1))
print("snapshot len:", len(snap_keys), len(c))

# --- equality, copy -------------------------------------------------------------
c2 = c.copy()
print("copy:", c2 is not c, c == c2, c != c2, len(c2))
print("eq other types:", c == 1, c == {}, c == None, c != 1)
attempt("ordering", lambda: c < c2)
print("unbound eq:", Context.__eq__(c, 1), Context.__eq__(c, c2), Context.__ne__(c, c2))
c2.run(a.set, "changed")
print("copy independent:", c[a], c2[a], c == c2)
c.run(b.set, "B2")
print("original independent:", c[b], c2[b])


class Always:
    def __eq__(self, other):
        return True

    __hash__ = None


class Never:
    def __eq__(self, other):
        return False

    __hash__ = None


shared = Never()
ce1, ce2, ce3 = Context(), Context(), Context()
ce1.run(a.set, shared)
ce2.run(a.set, shared)
ce3.run(a.set, Always())
print("eq identity shortcut:", ce1 == ce2, ce1 == ce3, ce3 == ce1)

# --- Context.run ----------------------------------------------------------------
cr = Context()
print("run result:", cr.run(lambda x, y=2, *rest, **kw: (x, y, rest, sorted(kw.items())), 1, 3, 4, k=5, callable=6))
attempt("run()", lambda: cr.run())
attempt("run(1)", lambda: cr.run(1))
attempt("run(callable=)", lambda: cr.run(callable=print))
attempt("unbound run", lambda: Context.run(1, print))
print("not entered after errors:", cr.run(lambda: "ok"))
rv = ContextVar("rv", default="d")
rv.set("outer")


def inner_run():
    seen = rv.get()
    rv.set("inner")
    return seen, rv.get()


print("run isolates:", cr.run(inner_run), rv.get(), cr[rv])
print("run persists:", cr.run(rv.get), rv.get())

# nested contexts restore the enclosing one
c_outer = Context()
c_inner = Context()


def outer_body():
    rv.set("o")
    got = c_inner.run(lambda: (rv.set("i"), rv.get())[1])
    return got, rv.get()


print("nested:", c_outer.run(outer_body), rv.get(), c_inner[rv], c_outer[rv])

# re-entering an entered context
c_re = Context()


def reenter():
    try:
        c_re.run(lambda: None)
    except RuntimeError as exc:
        return norm(exc)


print("reenter:", c_re.run(reenter))


def reenter_via_copy():
    return c_re.copy().run(lambda: "copy is a new context")


print("reenter copy:", c_re.run(reenter_via_copy))
print("after reenter:", c_re.run(lambda: "usable again"))

# exceptions unwind the context


class Boom(Exception):
    pass


c_exc = Context()


def boom():
    rv.set("set before boom")
    raise Boom("x")


attempt("run raises", lambda: c_exc.run(boom))
print("after exception:", rv.get(), c_exc[rv], c_exc.run(lambda: "re-entered"))
attempt("run raises non-callable", lambda: c_exc.run(42))
print("entered flag cleared:", c_exc.run(rv.get))

# copy_context
rv.set("before copy")
snap = copy_context()
rv.set("after copy")
print("copy_context:", snap[rv], rv.get(), snap.run(rv.get), type(snap).__name__, snap is not copy_context())
snap.run(rv.set, "snap only")
print("copy_context isolation:", rv.get(), snap[rv])
copy_inside = c_outer.run(copy_context)
print("copy inside run:", copy_inside[rv], copy_inside is not c_outer, copy_inside == c_outer)

# values may be unhashable / arbitrary
lst = ContextVar("lst")
lst_token = lst.set([1, 2])
lst.get().append(3)
print("mutable value:", lst.get())
lst.reset(lst_token)

# --- threads ----------------------------------------------------------------------
tv = ContextVar("tv", default="default")
tv.set("main")
thread_log = {}


def worker():
    thread_log["initial"] = (tv.get(), len(copy_context()))
    tok = tv.set("thread")
    thread_log["after set"] = tv.get()
    local = Context()
    thread_log["nested run"] = local.run(lambda: (tv.get(), tv.set("nested") and None, tv.get()))
    thread_log["restored"] = tv.get()
    tv.reset(tok)
    thread_log["after reset"] = tv.get()
    thread_log["main token reset"] = None
    try:
        tv.reset(main_token)
    except ValueError as exc:
        thread_log["main token reset"] = type(exc).__name__ + ": " + norm(exc).split(" was ")[1]


main_token = tv.set("main2")
t = threading.Thread(target=worker)
t.start()
t.join()
print("thread log:", sorted(thread_log.items()))
print("main untouched:", tv.get(), len(copy_context()) >= 1)
tv.reset(main_token)

# several threads at once never see each other's values
barrier = threading.Barrier(4)
results = {}


def racer(index):
    token = tv.set(index)
    barrier.wait()
    seen_before = tv.get()
    barrier.wait()
    results[index] = (seen_before, tv.get())
    tv.reset(token)


threads = [threading.Thread(target=racer, args=(i,)) for i in range(4)]
for th in threads:
    th.start()
for th in threads:
    th.join()
print("racers:", sorted(results.items()), tv.get())

# One Context entered by one thread cannot be entered by another
shared_ctx = Context()
entered = threading.Event()
release = threading.Event()
other_thread = {}


def holder():
    entered.set()
    release.wait(5)


def intruder():
    try:
        shared_ctx.run(lambda: None)
    except RuntimeError as exc:
        other_thread["error"] = norm(exc)
    other_thread["own current"] = tv.get()


def hold_in_main():
    th = threading.Thread(target=intruder)
    th.start()
    th.join()
    release.set()


shared_ctx.run(tv.set, "shared")
# the main thread enters `shared_ctx` and starts the intruder while inside it
shared_ctx.run(hold_in_main)
print("cross-thread enter:", sorted(other_thread.items()))
print("shared usable afterwards:", shared_ctx.run(tv.get))

# A Context made in one thread can be run in another
handoff = Context()
handoff.run(tv.set, "from main")
out = []
th = threading.Thread(target=lambda: out.append(handoff.run(tv.get)))
th.start()
th.join()
print("handoff:", out, tv.get())

# --- decimal uses a ContextVar --------------------------------------------------
import decimal

decimal.getcontext().prec = 28
dctx = copy_context()


def lower_precision():
    decimal.getcontext().prec = 5
    return str(decimal.Decimal(1) / decimal.Decimal(3))


print("decimal in context:", dctx.run(lower_precision), decimal.getcontext().prec, dctx.run(lambda: decimal.getcontext().prec))
print("decimal in fresh context:", Context().run(lambda: decimal.getcontext().prec))
with decimal.localcontext() as lc:
    lc.prec = 3
    print("localcontext:", decimal.Decimal(2) / decimal.Decimal(3))
    print("localcontext isolated:", Context().run(lambda: decimal.getcontext().prec))
print("localcontext restored:", decimal.Decimal(2) / decimal.Decimal(3))
dec_out = []


def dec_worker():
    dec_out.append(decimal.getcontext().prec)
    decimal.getcontext().prec = 7
    dec_out.append(decimal.getcontext().prec)


decimal.getcontext().prec = 11
th = threading.Thread(target=dec_worker)
th.start()
th.join()
print("decimal thread:", dec_out, decimal.getcontext().prec)
decimal.getcontext().prec = 28

print("contextvars native parity ok")
