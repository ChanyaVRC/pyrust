import abc
import functools
import inspect
import os
import sys
import types


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, str(e))


# 1. Module metadata and __all__
print("in sys.modules:", "inspect" in sys.modules)
for name in [
    "Signature", "Parameter", "BoundArguments", "signature",
    "ismodule", "isclass", "ismethod", "isfunction",
    "isgeneratorfunction", "isgenerator", "iscoroutinefunction", "iscoroutine",
    "isasyncgenfunction", "isasyncgen", "isawaitable",
    "istraceback", "isframe", "iscode", "isbuiltin", "ismethodwrapper",
    "isroutine", "isabstract", "ismethoddescriptor", "isdatadescriptor",
    "isgetsetdescriptor", "ismemberdescriptor",
    "getmembers", "getmembers_static", "getattr_static", "getmro", "getclasstree",
    "unwrap", "currentframe", "stack", "trace", "getframeinfo",
    "getouterframes", "getinnerframes", "FrameInfo", "Traceback",
    "getargvalues", "getclosurevars", "getgeneratorstate", "getcoroutinestate",
    "getdoc", "cleandoc", "getcomments", "getmodule", "getfile",
    "getsourcefile", "getabsfile", "getsource", "getsourcelines",
]:
    assert hasattr(inspect, name), f"missing {name}"
print("CO flags:", inspect.CO_OPTIMIZED, inspect.CO_NEWLOCALS, inspect.CO_VARARGS,
      inspect.CO_VARKEYWORDS, inspect.CO_GENERATOR, inspect.CO_COROUTINE,
      inspect.CO_ITERABLE_COROUTINE, inspect.CO_ASYNC_GENERATOR)


# 2. Type predicates
def plain_fn(x):
    return x


def gen_fn(x):
    yield x


async def coro_fn(x):
    return x


async def agen_fn(x):
    yield x


class Slotted:
    __slots__ = ("slot_x",)


class DataDesc:
    def __get__(self, obj, tp=None):
        return 1

    def __set__(self, obj, val):
        pass


class NonDataDesc:
    def __get__(self, obj, tp=None):
        return 2


class MyABC(abc.ABC):
    @abc.abstractmethod
    def req(self):
        pass


class MyConcrete(MyABC):
    def req(self):
        return 1

    def inst_method(self, a):
        return a

    @classmethod
    def cls_method(cls, a):
        return a

    @staticmethod
    def static_method(a):
        return a

    @property
    def prop(self):
        return 42


class CustomAwaitable:
    def __await__(self):
        return iter(())


@inspect.markcoroutinefunction
def marked_coro():
    pass


g = gen_fn(1)
c = coro_fn(1)
ag = agen_fn(1)
inst = MyConcrete()
cur_frame = inspect.currentframe()
try:
    1 / 0
except ZeroDivisionError as ze:
    sample_tb = ze.__traceback__

print("ismodule:", inspect.ismodule(sys), inspect.ismodule(MyConcrete), inspect.ismodule(1))
print("isclass:", inspect.isclass(MyConcrete), inspect.isclass(int), inspect.isclass(inst))
print("ismethod:", inspect.ismethod(inst.inst_method), inspect.ismethod(MyConcrete.cls_method),
      inspect.ismethod(MyConcrete.inst_method), inspect.ismethod(plain_fn))
print("isfunction:", inspect.isfunction(plain_fn), inspect.isfunction(MyConcrete.inst_method),
      inspect.isfunction(inst.inst_method), inspect.isfunction(len))
print("isgeneratorfunction:", inspect.isgeneratorfunction(gen_fn),
      inspect.isgeneratorfunction(functools.partial(gen_fn, 1)),
      inspect.isgeneratorfunction(plain_fn), inspect.isgeneratorfunction(agen_fn))
print("isgenerator:", inspect.isgenerator(g), inspect.isgenerator(c), inspect.isgenerator(ag))
print("iscoroutinefunction:", inspect.iscoroutinefunction(coro_fn),
      inspect.iscoroutinefunction(functools.partial(coro_fn, 1)),
      inspect.iscoroutinefunction(marked_coro),
      inspect.iscoroutinefunction(plain_fn), inspect.iscoroutinefunction(agen_fn))
print("iscoroutine:", inspect.iscoroutine(c), inspect.iscoroutine(g), inspect.iscoroutine(ag))
print("isasyncgenfunction:", inspect.isasyncgenfunction(agen_fn),
      inspect.isasyncgenfunction(functools.partial(agen_fn, 1)),
      inspect.isasyncgenfunction(gen_fn), inspect.isasyncgenfunction(coro_fn))
print("isasyncgen:", inspect.isasyncgen(ag), inspect.isasyncgen(g), inspect.isasyncgen(c))
print("isawaitable:", inspect.isawaitable(c), inspect.isawaitable(CustomAwaitable()),
      inspect.isawaitable(g), inspect.isawaitable(1))
print("istraceback:", inspect.istraceback(sample_tb), inspect.istraceback(cur_frame))
print("isframe:", inspect.isframe(cur_frame), inspect.isframe(sample_tb))
print("iscode:", inspect.iscode(plain_fn.__code__), inspect.iscode(plain_fn))
print("isbuiltin:", inspect.isbuiltin(len), inspect.isbuiltin([].append),
      inspect.isbuiltin([].__len__), inspect.isbuiltin(plain_fn))
print("ismethodwrapper:", inspect.ismethodwrapper([].__len__), inspect.ismethodwrapper([].append),
      inspect.ismethodwrapper(len))
print("isroutine:", inspect.isroutine(plain_fn), inspect.isroutine(inst.inst_method),
      inspect.isroutine(len), inspect.isroutine([].append), inspect.isroutine([].__len__),
      inspect.isroutine(list.append), inspect.isroutine(MyConcrete), inspect.isroutine(1))
print("isabstract:", inspect.isabstract(MyABC), inspect.isabstract(MyConcrete),
      inspect.isabstract(abc.ABC), inspect.isabstract(abc.ABCMeta), inspect.isabstract(int),
      inspect.isabstract(inst))
print("ismethoddescriptor:", inspect.ismethoddescriptor(list.append),
      inspect.ismethoddescriptor(int.__add__),
      inspect.ismethoddescriptor(NonDataDesc()),
      inspect.ismethoddescriptor(DataDesc()),
      inspect.ismethoddescriptor(plain_fn),
      inspect.ismethoddescriptor(inst.inst_method))
print("isdatadescriptor:", inspect.isdatadescriptor(MyConcrete.prop),
      inspect.isdatadescriptor(DataDesc()),
      inspect.isdatadescriptor(NonDataDesc()),
      inspect.isdatadescriptor(Slotted.slot_x),
      inspect.isdatadescriptor(int.real))
print("isgetsetdescriptor:", inspect.isgetsetdescriptor(int.real),
      inspect.isgetsetdescriptor(Slotted.slot_x))
print("ismemberdescriptor:", inspect.ismemberdescriptor(Slotted.slot_x),
      inspect.ismemberdescriptor(int.real))

c.close()


# 3. Parameter, Signature, BoundArguments, signature()
P = inspect.Parameter
p1 = P("a", P.POSITIONAL_ONLY)
p2 = P("b", P.POSITIONAL_OR_KEYWORD, default=10, annotation=int)
p3 = P("args", P.VAR_POSITIONAL)
p4 = P("c", P.KEYWORD_ONLY, default="x")
p5 = P("kwargs", P.VAR_KEYWORD, annotation=str)
print("Parameter kinds:", int(P.POSITIONAL_ONLY), str(P.POSITIONAL_ONLY),
      P.POSITIONAL_ONLY.description, P.VAR_KEYWORD.description)
print("Parameter str/repr:", str(p1), str(p2), str(p3), str(p4), str(p5), repr(p2))
print("Parameter replace:", str(p2.replace(name="bb", default=20)))
assert p2 == P("b", P.POSITIONAL_OR_KEYWORD, default=10, annotation=int)
assert hash(p2) == hash(P("b", P.POSITIONAL_OR_KEYWORD, default=10, annotation=int))
print("Parameter bad default:", expect_exc(P, "args", P.VAR_POSITIONAL, default=1))
print("Parameter bad name:", expect_exc(P, "123", P.POSITIONAL_OR_KEYWORD))
print("Parameter kw name pos-only ok:", P("class", P.POSITIONAL_ONLY).name)
print("Parameter kw name non-pos-only err:", expect_exc(P, "class", P.POSITIONAL_OR_KEYWORD))

sig_manual = inspect.Signature([p1, p2, p3, p4, p5], return_annotation=bool)
print("Signature manual str:", str(sig_manual))
print("Signature manual repr:", repr(sig_manual))
assert isinstance(sig_manual.parameters, types.MappingProxyType)
assert list(sig_manual.parameters.keys()) == ["a", "b", "args", "c", "kwargs"]
assert sig_manual.return_annotation is bool
print("Signature bad order:", expect_exc(inspect.Signature, [p4, p1]))
print("Signature non-default after default:",
      expect_exc(inspect.Signature, [P("x", P.POSITIONAL_OR_KEYWORD, default=1), P("y", P.POSITIONAL_OR_KEYWORD)]))
print("Signature dup name:",
      expect_exc(inspect.Signature, [P("x", P.POSITIONAL_OR_KEYWORD), P("x", P.KEYWORD_ONLY)]))


def full_fn(a: int, b: str = "hi", /, c=2, *args: float, d: bool, e=3, **kwargs) -> list[int]:
    return []


sig_full = inspect.signature(full_fn)
print("signature full:", str(sig_full))
assert list(sig_full.parameters.keys()) == ["a", "b", "c", "args", "d", "e", "kwargs"]
assert sig_full.parameters["a"].kind is P.POSITIONAL_ONLY
assert sig_full.parameters["b"].kind is P.POSITIONAL_ONLY
assert sig_full.parameters["b"].default == "hi"
assert sig_full.parameters["c"].kind is P.POSITIONAL_OR_KEYWORD
assert sig_full.parameters["args"].kind is P.VAR_POSITIONAL
assert sig_full.parameters["d"].kind is P.KEYWORD_ONLY
assert sig_full.parameters["d"].default is P.empty
assert sig_full.parameters["e"].default == 3
assert sig_full.parameters["kwargs"].kind is P.VAR_KEYWORD
assert sig_full.return_annotation == list[int]


def kwonly_star(x, *, y=1, z):
    pass


print("signature kwonly_star:", str(inspect.signature(kwonly_star)))


# eval_str=True with stringized annotations
def str_ann(x: "int", y: "CustomLocal") -> "str":
    return ""


sig_no_eval = inspect.signature(str_ann)
sig_eval = inspect.signature(str_ann, locals={"CustomLocal": float}, eval_str=True)
print("eval_str=False:", sig_no_eval.parameters["x"].annotation, sig_no_eval.return_annotation)
print("eval_str=True:", sig_eval.parameters["x"].annotation, sig_eval.parameters["y"].annotation,
      sig_eval.return_annotation)


# Bound methods, classmethods, staticmethods, classes, callables, wraps, partial
class SampleCls:
    def __init__(self, x, y=5, *, flag=False):
        self.x = x

    def method(self, a, b=1):
        return a + b

    @classmethod
    def cm(cls, k, v=2):
        return (cls, k, v)

    @staticmethod
    def sm(p, q=3):
        return p + q


class SubCls(SampleCls):
    pass


class NewCls:
    def __new__(cls, m, n=7):
        return super().__new__(cls)


class EmptyCls:
    pass


class CallableObj:
    def __call__(self, u, /, v=9):
        return u + v


class MetaCall(type):
    def __call__(cls, z, w=4):
        return super().__call__()


class WithMeta(metaclass=MetaCall):
    pass


s_inst = SampleCls(1)
print("sig bound method:", str(inspect.signature(s_inst.method)))
print("sig unbound method:", str(inspect.signature(SampleCls.method)))
print("sig classmethod:", str(inspect.signature(SampleCls.cm)))
print("sig staticmethod:", str(inspect.signature(SampleCls.sm)))
print("sig class __init__:", str(inspect.signature(SampleCls)))
print("sig subclass __init__:", str(inspect.signature(SubCls)))
print("sig class __new__:", str(inspect.signature(NewCls)))
print("sig empty class:", str(inspect.signature(EmptyCls)))
print("sig callable obj:", str(inspect.signature(CallableObj())))
print("sig metaclass __call__:", str(inspect.signature(WithMeta)))


def dec(fn):
    @functools.wraps(fn)
    def wrapper(*args, **kwargs):
        return fn(*args, **kwargs)
    return wrapper


@dec
def wrapped_target(x, y=10):
    return x + y


print("sig follow_wrapped=True:", str(inspect.signature(wrapped_target)))
print("sig follow_wrapped=False:", str(inspect.signature(wrapped_target, follow_wrapped=False)))


def custom_sig_fn(*a):
    pass


custom_sig_fn.__signature__ = inspect.Signature([P("custom", P.POSITIONAL_OR_KEYWORD, default=99)])
print("sig __signature__:", str(inspect.signature(custom_sig_fn)))


def partial_target(a, /, b, c, *args, d=4, **kwargs):
    pass


p_pos = functools.partial(partial_target, 1, 2)
p_kw = functools.partial(partial_target, 1, b=20)
p_nested = functools.partial(p_pos, c=30)
print("sig partial pos:", str(inspect.signature(p_pos)))
print("sig partial kw:", str(inspect.signature(p_kw)))
print("sig partial nested:", str(inspect.signature(p_nested)))


# BoundArguments
def bind_fn(a, /, b, c=3, *args, d, e=5, **kwargs):
    pass


sig_b = inspect.signature(bind_fn)
ba = sig_b.bind(1, 2, 30, 40, 50, d=6, extra=7)
print("bind arguments:", ba.arguments)
print("bind args:", ba.args)
print("bind kwargs:", ba.kwargs)
ba.apply_defaults()
print("bind after apply_defaults:", ba.arguments)

ba_part = sig_b.bind_partial(1, c=30)
print("bind_partial:", ba_part.arguments, ba_part.args, ba_part.kwargs)
ba_part.apply_defaults()
print("bind_partial apply_defaults:", ba_part.arguments)

def _posonly(x, /):
    return x


def _posonly_def(x=1, /):
    return x


print("bind missing pos-only:", expect_exc(sig_b.bind))
print("bind missing kw-only:", expect_exc(sig_b.bind, 1, 2))
print("bind pos-only as kw:", expect_exc(inspect.signature(_posonly).bind, x=1)[0])
print("bind pos-only def as kw:", expect_exc(inspect.signature(_posonly_def).bind, x=2)[0])
print("bind too many pos:", expect_exc(inspect.signature(lambda x: x).bind, 1, 2))
print("bind multiple values:", expect_exc(inspect.signature(lambda x, y: x).bind, 1, x=2))
print("bind unexpected kw:", expect_exc(inspect.signature(lambda x: x).bind, 1, z=9))


# 4. getmembers, getmembers_static, getattr_static, getmro, getclasstree, unwrap
class StaticDemo:
    triggered = 0

    @property
    def boom(self):
        StaticDemo.triggered += 1
        return "evaluated"

    def hello(self):
        pass

    x = 123


sd = StaticDemo()
static_val = inspect.getattr_static(sd, "boom")
assert isinstance(static_val, property)
assert StaticDemo.triggered == 0
static_members = dict(inspect.getmembers_static(sd))
assert isinstance(static_members["boom"], property)
assert StaticDemo.triggered == 0
dyn_members = dict(inspect.getmembers(sd, predicate=inspect.ismethod))
print("getmembers ismethod:", sorted(dyn_members.keys()))
print("getattr_static triggered:", StaticDemo.triggered)


class A:
    pass


class B(A):
    pass


class C(A):
    pass


class D(B, C):
    pass


print("getmro:", [cls.__name__ for cls in inspect.getmro(D)])
tree_nonuniq = inspect.getclasstree([A, B, C, D], unique=False)
tree_uniq = inspect.getclasstree([A, B, C, D], unique=True)
assert tree_nonuniq != tree_uniq
print("getclasstree ok:", len(tree_nonuniq), len(tree_uniq))


def u0():
    pass


def u1():
    pass


def u2():
    pass


u1.__wrapped__ = u0
u2.__wrapped__ = u1
assert inspect.unwrap(u2) is u0
assert inspect.unwrap(u2, stop=lambda f: f is u1) is u1
cyc1 = lambda: None
cyc2 = lambda: None
cyc1.__wrapped__ = cyc2
cyc2.__wrapped__ = cyc1
print("unwrap cycle:", expect_exc(inspect.unwrap, cyc1)[0])


# 5. Frame, stack, trace, getargvalues, getclosurevars, generator/coroutine states
def frame_inner(p, q=2, *va, **kw):
    cf = inspect.currentframe()
    st = inspect.stack()
    av = inspect.getargvalues(cf)
    fi = inspect.getframeinfo(cf)
    return (
        cf.f_code.co_name,
        [info.function for info in st[:3]],
        av.args,
        av.varargs,
        av.keywords,
        av.locals["p"],
        av.locals["q"],
        av.locals["va"],
        av.locals["kw"],
        fi.function,
        isinstance(fi, inspect.Traceback),
        isinstance(st[0], inspect.FrameInfo),
    )


def frame_outer():
    return frame_inner(10, 20, 30, extra=40)


print("frame/stack/argvalues:", frame_outer())

try:
    def raise_here():
        raise ValueError("trace me")
    raise_here()
except ValueError:
    tr = inspect.trace()
    print("trace functions:", [t.function for t in tr])


GLOBAL_CONST = 777


def make_closure(n):
    captured = n + 1

    def inner(obj):
        return captured + GLOBAL_CONST + len(obj) + obj.missing_attr

    return inner


cl_fn = make_closure(5)
cv = inspect.getclosurevars(cl_fn)
print("getclosurevars:", cv.nonlocals, sorted(cv.globals.keys()), sorted(cv.builtins.keys()), sorted(cv.unbound))


# Generator / coroutine / asyncgen states
gen_states = []


def stateful_gen():
    gen_states.append(inspect.getgeneratorstate(sg))
    yield 1
    yield 2


sg = stateful_gen()
gen_states.append(inspect.getgeneratorstate(sg))
next(sg)
gen_states.append(inspect.getgeneratorstate(sg))
sg.close()
gen_states.append(inspect.getgeneratorstate(sg))
print("generator states:", gen_states)

sc = coro_fn(1)
c_created = inspect.getcoroutinestate(sc)
sc.close()
c_closed = inspect.getcoroutinestate(sc)
print("coroutine states:", c_created, c_closed)

sag = agen_fn(1)
print("asyncgen state created:", inspect.getasyncgenstate(sag))


# 6. Docstring & source helpers
raw_doc = """First line.

        Second line indented.
            Third line more indented.

    """
print("cleandoc:", repr(inspect.cleandoc(raw_doc)))
print("indentsize:", inspect.indentsize("    abc"), inspect.indentsize("\tabc"))


class DocBase:
    """Base doc.

    Details here.
    """

    def documented(self):
        """Method doc."""


class DocSub(DocBase):
    def documented(self):
        pass


print("getdoc base:", repr(inspect.getdoc(DocBase)))
print("getdoc sub inherits:", repr(inspect.getdoc(DocSub)))
print("getdoc method inherits:", repr(inspect.getdoc(DocSub.documented)))
print("getdoc none:", inspect.getdoc(plain_fn))


# Preceding comment line 1
# Preceding comment line 2
def commented_fn(x):
    return x + 1


print("getcomments:", repr(inspect.getcomments(commented_fn)))
print("getmodule fn:", inspect.getmodule(commented_fn).__name__)
print("getmodulename:", inspect.getmodulename("/tmp/pkg/mymod.py"))
src_file = inspect.getsourcefile(commented_fn)
assert src_file is not None and os.path.basename(src_file) == "test_inspect.py"
assert os.path.basename(inspect.getfile(commented_fn)) == "test_inspect.py"
assert os.path.isabs(inspect.getabsfile(commented_fn))
src_lines, start_lno = inspect.getsourcelines(commented_fn)
print("getsource fn:", repr(inspect.getsource(commented_fn)))
assert "".join(src_lines) == inspect.getsource(commented_fn)
assert start_lno > 0
print("source helpers ok")
