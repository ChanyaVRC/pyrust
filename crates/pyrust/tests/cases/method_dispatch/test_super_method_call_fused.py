# Zero-argument `super().name(...)` calls (#3424).
#
# The compiler fuses `super().name(args)` into a method load that skips the
# `super` object when `super` is the real builtin and the target is a plain
# method, with a per-site cache keyed by the receiver class and the defining
# class. Every case below must print exactly what CPython 3.12 prints: the
# fast path, each reason for leaving it, and each way of invalidating it.

import builtins


def section(title):
    print(f"--- {title}")


# ---------------------------------------------------------------- basics
section("single inheritance")


class A:
    def f(self, x):
        return ("A.f", x)

    def g(self):
        return "A.g"

    def h(self, a, b=10, *rest, key=None):
        return ("A.h", a, b, rest, key)


class B(A):
    def f(self, x):
        return ("B.f", super().f(x))

    def g(self):
        return "B.g>" + super().g()

    def h(self, a, b=20):
        return ("B.h", super().h(a), super().h(a, b), super().h(a, b, 3, 4))


b = B()
for i in range(4):
    print(b.f(i), b.g())
print(b.h(1))
print(b.h(1, 2))

# ---------------------------------------------------------------- MRO
section("diamond MRO")


class Root:
    def who(self, trail):
        return trail + ["Root"]


class Left(Root):
    def who(self, trail):
        return super().who(trail + ["Left"])


class Right(Root):
    def who(self, trail):
        return super().who(trail + ["Right"])


class Bottom(Left, Right):
    def who(self, trail):
        return super().who(trail + ["Bottom"])


class BottomSwapped(Right, Left):
    def who(self, trail):
        return super().who(trail + ["BottomSwapped"])


for _ in range(3):
    # The same `super().who` site in `Left` resolves to `Right.who` for a
    # `Bottom` and to `Root.who` for a plain `Left` or a `BottomSwapped`.
    print(Bottom().who([]))
    print(Left().who([]))
    print(BottomSwapped().who([]))
    print(Right().who([]))
print([c.__name__ for c in Bottom.__mro__])

section("polymorphic receiver classes at one site")


class Base:
    def tag(self):
        return "Base"


class Mid(Base):
    def tag(self):
        return type(self).__name__ + ":" + super().tag()


leaves = [type(f"Leaf{i}", (Mid,), {}) for i in range(12)]
for _ in range(2):
    print([leaf().tag() for leaf in leaves])


class LeafOverride(Mid):
    def tag(self):
        return "LeafOverride>" + super().tag()


print(LeafOverride().tag())

section("deep chain and __init__")


class P0:
    def __init__(self, log):
        log.append("P0")
        self.log = log


class P1(P0):
    def __init__(self, log):
        log.append("P1")
        super().__init__(log)


class P2(P1):
    def __init__(self, log):
        log.append("P2")
        super().__init__(log)


class P3(P2):
    def __init__(self, log):
        log.append("P3")
        super().__init__(log)


for _ in range(3):
    print(P3([]).log, P2([]).log)

# ---------------------------------------------------------------- non-plain targets
section("property, classmethod, staticmethod, descriptors")


class Getter:
    def __get__(self, obj, owner):
        return lambda *a: ("Getter", type(obj).__name__, owner.__name__, a)


class D0:
    @property
    def prop(self):
        return "D0.prop"

    @property
    def callable_prop(self):
        return lambda x: ("D0.callable_prop", x)

    @classmethod
    def cm(cls, x):
        return ("D0.cm", cls.__name__, x)

    @staticmethod
    def sm(x):
        return ("D0.sm", x)

    desc = Getter()
    plain_attr = len

    def __repr__(self):
        return "D0-repr"


class D1(D0):
    @property
    def prop(self):
        return "D1.prop>" + super().prop

    def call_prop(self):
        return super().callable_prop(5)

    @classmethod
    def cm(cls, x):
        return ("D1.cm", super().cm(x))

    def cm_from_instance(self, x):
        return super().cm(x)

    @staticmethod
    def sm(x):
        return ("D1.sm", x)

    def sm_from_instance(self, x):
        return super().sm(x)

    def desc_call(self):
        return super().desc(1, 2)

    def attr_call(self):
        return super().plain_attr("four")

    def __repr__(self):
        return "D1>" + super().__repr__()


class D2(D1):
    pass


for obj in (D1(), D2(), D1()):
    print(obj.prop, obj.call_prop())
    print(obj.cm(1), type(obj).cm(2), obj.cm_from_instance(3))
    print(obj.sm_from_instance(4))
    print(obj.desc_call(), obj.attr_call())
    print(repr(obj))

section("builtin and object bases")


class MyList(list):
    def append(self, item):
        super().append(item * 2)

    def __len__(self):
        return super().__len__() + 100

    def describe(self):
        return super().__repr__()


class MyDict(dict):
    def __setitem__(self, key, value):
        super().__setitem__(key, (value, "wrapped"))

    def get(self, key, default=None):
        return ("get", super().get(key, default))


class MyError(ValueError):
    def __init__(self, msg, code):
        super().__init__(msg)
        self.code = code

    def __str__(self):
        return f"[{self.code}] {self.args[0]}"


class Plain:
    def __init__(self):
        super().__init__()
        self.ready = True

    def __eq__(self, other):
        return super().__eq__(other)

    def __hash__(self):
        return 7

    def __setattr__(self, name, value):
        super().__setattr__(name, value)


for _ in range(3):
    ml = MyList()
    ml.append(1)
    ml.append("ab")
    print(ml.describe(), len(ml))
    md = MyDict()
    md["k"] = 1
    print(md, md.get("k"), md.get("missing", 0))
    err = MyError("boom", 3)
    print(str(err), err.args, err.code)
    p = Plain()
    print(p.ready, p == p, p == Plain())

# ---------------------------------------------------------------- shadowing `super`
section("module-level rebinding of super")


class S0:
    def f(self):
        return "S0.f"


class S1(S0):
    def f(self):
        return super().f()


s1 = S1()
print(s1.f(), s1.f())


class FakeSuper:
    def f(self):
        return "FakeSuper.f"


real_super = super
super = FakeSuper
print(s1.f())
super = lambda: [].__class__  # noqa: E731
try:
    print(s1.f())
except AttributeError as exc:
    print("AttributeError:", exc)
super = lambda: S0()  # noqa: E731 -- an instance, like the fast path's receiver
print(s1.f())
del super
print(s1.f(), super is real_super)

section("builtins.super patch")


def patched_super(*args):
    print("patched_super called with", len(args), "args")
    return FakeSuper()


builtins.super = patched_super
try:
    print(s1.f())
finally:
    builtins.super = real_super
print(s1.f())

section("local, parameter, and closure named super")


class S2(S0):
    def local_shadow(self):
        super = lambda: FakeSuper()  # noqa: E731
        return super().f()

    def param_shadow(self, super):
        return super().f()

    def closure_shadow(self):
        super = FakeSuper  # noqa: F841

        def inner():
            return super().f()

        return inner()

    def global_decl(self):
        global super
        return super().f()


s2 = S2()
print(s2.local_shadow(), s2.param_shadow(FakeSuper), s2.closure_shadow())
print(s2.global_decl())

# ---------------------------------------------------------------- monkey-patching
section("patching the target method")


class M0:
    def f(self):
        return "M0.f"


class M1(M0):
    pass


class M2(M1):
    def f(self):
        return "M2>" + super().f()


m2 = M2()
print(m2.f(), m2.f())
M0.f = lambda self: "M0.patched"
print(m2.f())
M1.f = lambda self: "M1.inserted"
print(m2.f())
del M1.f
print(m2.f())
M0.f = property(lambda self: (lambda: "M0.property"))
print(m2.f())
M0.f = staticmethod(lambda: "M0.static")
print(m2.f())
M0.f = classmethod(lambda cls: "M0.class:" + cls.__name__)
print(m2.f())
M0.f = lambda self: "M0.plain-again"
print(m2.f(), m2.f())
del M0.f
try:
    m2.f()
except AttributeError as exc:
    print("AttributeError:", exc)
M0.f = lambda self: "M0.restored"
print(m2.f())

section("patching through setattr and delattr")
setattr(M1, "f", lambda self: "M1.setattr")
print(m2.f())
delattr(M1, "f")
print(m2.f())

section("reassigning __class__ on the receiver")


class Q0:
    def f(self):
        return "Q0.f"


class Q1(Q0):
    def f(self):
        return "Q1>" + super().f()


class Q2(Q1):
    pass


class QX(Q1):
    def f(self):
        return "QX>" + super().f()


q = Q1()
print(q.f())
q.__class__ = Q2
print(q.f())
q.__class__ = QX
print(q.f())

section("instance attribute does not shadow super lookup")
m2.f2 = "instance"


class I0:
    def f(self):
        return "I0.f"


class I1(I0):
    def f(self):
        return "I1>" + super().f()


i1 = I1()
i1.f = lambda: "instance-level"
print(I1.f(i1), i1.f())

# ---------------------------------------------------------------- scopes
section("comprehensions, closures, lambdas, generators")


class C0:
    def val(self, x):
        return x * 10

    def name(self):
        return "C0"


class C1(C0):
    def val(self, x):
        return super().val(x) + 1

    def listcomp(self):
        return [super().val(i) for i in range(3)]

    def nested_listcomp(self):
        return [[super().val(i + j) for j in range(2)] for i in range(2)]

    def setcomp(self):
        return sorted({super().val(i) for i in range(3)})

    def dictcomp(self):
        return {i: super().val(i) for i in range(3)}

    def genexpr(self):
        try:
            return list(super().val(i) for i in range(3))
        except TypeError as exc:
            return "TypeError: " + str(exc)

    def closure(self):
        def inner(me):
            return super().val(7)

        return inner(self)

    def closure_no_arg(self):
        def inner():
            return super().val(7)

        try:
            return inner()
        except RuntimeError as exc:
            return "RuntimeError: " + str(exc)

    def lambda_call(self):
        return (lambda me: super().val(8))(self)

    def generator(self):
        for i in range(3):
            yield super().val(i)

    def conditional(self, flag):
        return super().val(1) if flag else super().name()

    def chained(self):
        return super().name().lower() + super().name()

    def as_argument(self):
        return super().val(super().val(super().val(1)))


c1 = C1()
for _ in range(2):
    print(c1.val(2), c1.listcomp(), c1.nested_listcomp(), c1.setcomp(), c1.dictcomp())
    print(c1.genexpr())
    print(c1.closure(), c1.closure_no_arg(), c1.lambda_call(), list(c1.generator()))
    print(c1.conditional(True), c1.conditional(False), c1.chained(), c1.as_argument())

section("async methods")
import asyncio  # noqa: E402


class As0:
    async def fetch(self, x):
        return ("As0", x)

    def sync(self, x):
        return ("As0.sync", x)


class As1(As0):
    async def fetch(self, x):
        first = await super().fetch(x)
        return ("As1", first, super().sync(x))


print(asyncio.run(As1().fetch(1)))

# ---------------------------------------------------------------- rebinding self
section("rebinding the first argument")


class R0:
    def f(self):
        return "R0.f on " + type(self).__name__


class R1(R0):
    def f(self):
        return "R1>" + super().f()

    def rebind(self, other):
        self = other
        return super().f()

    def rebind_to_int(self):
        self = 5
        try:
            return super().f()
        except TypeError as exc:
            return "TypeError: " + str(exc)

    def captured(self):
        def peek():
            return self

        return super().f() + "|" + type(peek()).__name__


class R2(R1):
    pass


class Unrelated:
    pass


r1 = R1()
print(r1.rebind(R2()), r1.rebind(R1()))
try:
    r1.rebind(Unrelated())
except TypeError as exc:
    print("TypeError:", exc)
print(r1.rebind_to_int(), r1.captured())

section("unbound call with a foreign receiver")
try:
    R1.f(Unrelated())
except TypeError as exc:
    print("TypeError:", exc)
print(R1.f(R2()))

# ---------------------------------------------------------------- errors
section("errors raised by super() itself")


class E0:
    def f(self, x):
        return x


class E1(E0):
    @staticmethod
    def static_no_args():
        return super().f(1)

    @staticmethod
    def static_with_arg(x):
        try:
            return super().f(x)
        except TypeError as exc:
            return "TypeError: " + str(exc)

    def varargs_only(*args):
        try:
            return super().f(1)
        except RuntimeError as exc:
            return "RuntimeError: " + str(exc)

    def kwonly(*, key):
        try:
            return super().f(1)
        except RuntimeError as exc:
            return "RuntimeError: " + str(exc)


def module_function(obj):
    return super().f(1)


try:
    E1.static_no_args()
except RuntimeError as exc:
    print("RuntimeError:", exc)
print(E1.static_with_arg(3))
print(E1().varargs_only())
print(E1.kwonly(key=1))
try:
    module_function(E1())
except RuntimeError as exc:
    print("RuntimeError:", exc)

section("argument evaluation order")


def note(label, value=None):
    print("  eval", label)
    return value


class O0:
    def f(self, a, b):
        return (a, b)


class O1(O0):
    def f(self, a, b):
        return super().f(note("a", a), note("b", b))

    def missing(self):
        return super().nope()

    @staticmethod
    def no_frame_arg():
        return super().f(note("never"), 2)


print(O1().f(1, 2))
try:
    O1().missing()
except AttributeError as exc:
    print("AttributeError:", exc)
try:
    O1.no_frame_arg()
except RuntimeError as exc:
    print("RuntimeError:", exc)

section("wrong arity and exceptions through the call")


class W0:
    def f(self, x):
        if x < 0:
            raise ValueError("negative")
        return x

    def no_args(self):
        return "ok"

    def defaults(self, a, b=2, c=3):
        return (a, b, c)


class W1(W0):
    def f(self, x):
        return super().f(x)

    def too_many(self):
        return super().no_args(1, 2)

    def too_few(self):
        return super().f()

    def defaults(self, a):
        return (super().defaults(a), super().defaults(a, 20), super().defaults(a, 20, 30))

    def guarded(self, x):
        try:
            return super().f(x)
        except ValueError as exc:
            return "caught " + str(exc)
        finally:
            print("  finally", x)


w1 = W1()
print(w1.f(3), w1.defaults(1))
for call in (lambda: w1.f(-1), w1.too_many, w1.too_few):
    try:
        call()
    except (ValueError, TypeError) as exc:
        print(type(exc).__name__ + ":", exc)
print(w1.guarded(1), w1.guarded(-1))

section("recursion through super")


class Rec0:
    def depth(self, n):
        return n


class Rec1(Rec0):
    def depth(self, n):
        if n >= 50:
            return super().depth(n)
        return self.depth(n + 1)

    def overflow(self):
        return super().overflow_target()


class Rec2(Rec1):
    def overflow_target(self):
        return self.overflow()


print(Rec1().depth(0))
try:
    Rec2().overflow()
except (RecursionError, AttributeError) as exc:
    print(type(exc).__name__)

# ---------------------------------------------------------------- classes built dynamically
section("same code object, several classes")


def make_pair(label):
    class Parent:
        def f(self):
            return label + ".Parent"

    class Child(Parent):
        def f(self):
            return label + ".Child>" + super().f()

    return Child


children = [make_pair(f"p{i}") for i in range(11)]
for _ in range(2):
    print([child().f() for child in children])


def shared_method(self):
    return "shared>" + super().f()


class T0:
    def f(self):
        return "T0.f"


class T1(T0):
    f = shared_method


try:
    T1().f()
except RuntimeError as exc:
    print("RuntimeError:", exc)

section("metaclass and classmethod receivers")


class Meta(type):
    def create(cls, x):
        return ("Meta.create", cls.__name__, x)

    def __call__(cls, *args):
        return ("Meta.__call__", super().__call__(*args).value)


class SubMeta(Meta):
    def create(cls, x):
        return ("SubMeta.create", super().create(x))


class WithMeta(metaclass=SubMeta):
    def __init__(self, value=0):
        self.value = value


print(WithMeta.create(1), WithMeta(5))


class K0:
    @classmethod
    def build(cls, x):
        return (cls.__name__, x)


class K1(K0):
    @classmethod
    def build(cls, x):
        return ("K1", super().build(x))


class K2(K1):
    pass


for _ in range(2):
    print(K1.build(1), K2.build(2), K2().build(3))

section("__getattribute__ and __getattr__ are bypassed by super")


class G0:
    def f(self):
        return "G0.f"


class G1(G0):
    def __getattribute__(self, name):
        if name == "f":
            return lambda: "from __getattribute__"
        return object.__getattribute__(self, name)

    def __getattr__(self, name):
        return lambda: "from __getattr__"

    def via_super(self):
        return super().f()

    def missing_via_super(self):
        try:
            return super().absent()
        except AttributeError as exc:
            return "AttributeError: " + str(exc)


g1 = G1()
print(g1.f(), G1.via_super(g1), G1.missing_via_super(g1))

section("super object special attributes")


class X0:
    def f(self):
        return "X0.f"


class X1(X0):
    def specials(self):
        return (
            super().__thisclass__.__name__,
            type(super().__self__).__name__,
            super().__self_class__.__name__,
            super().__class__.__name__,
        )


print(X1().specials())

section("functools.wraps / decorated targets and bound-method attributes")
import functools  # noqa: E402


def traced(func):
    @functools.wraps(func)
    def wrapper(self, *args):
        return ("traced", func(self, *args))

    return wrapper


class F0:
    @traced
    def f(self, x):
        return x + 1


class F1(F0):
    def f(self, x):
        return super().f(x)

    def bound(self):
        method = super().f
        return method.__name__, method.__self__ is self, method(1)


f1 = F1()
print(f1.f(1), f1.f(2), f1.bound())

section("slots, weakly typed receivers, and subclass of exception")


class Slotted:
    __slots__ = ("x",)

    def setx(self, x):
        self.x = x
        return self


class SlottedChild(Slotted):
    __slots__ = ("y",)

    def setx(self, x):
        self.y = x * 2
        return super().setx(x)


sc = SlottedChild().setx(4)
print(sc.x, sc.y)


class AppError(Exception):
    def describe(self):
        return "AppError"


class SpecificError(AppError):
    def describe(self):
        return "Specific>" + super().describe()


try:
    raise SpecificError("x")
except AppError as exc:
    print(exc.describe())

# ---------------------------------------------------------------- lifetimes
section("finalizers and weakrefs around super calls")
import weakref  # noqa: E402


class Fin0:
    def step(self, log):
        log.append("Fin0.step")
        return len(log)

    def chain(self):
        return self


class Fin1(Fin0):
    def step(self, log):
        log.append("Fin1.step")
        return super().step(log)

    def chain(self):
        return super().chain()

    def drop_self(self, log):
        result = super().step(log)
        self = None
        log.append("after self = None")
        return result

    def __del__(self):
        print("  __del__", type(self).__name__)


def run_finalizers():
    log = []
    print(Fin1().step(log))
    print(log)
    Fin1().chain()
    print("after chain")
    print(Fin1().drop_self(log), log[-1])
    print("end run_finalizers")


run_finalizers()
print("after run_finalizers")


class Weak0:
    def touch(self):
        return "Weak0.touch"


class Weak1(Weak0):
    def touch(self):
        return "Weak1>" + super().touch()


def run_weakrefs():
    obj = Weak1()
    ref = weakref.ref(obj, lambda r: print("  weakref callback"))
    print(obj.touch())
    del obj
    print("dead:", ref() is None)
    print(Weak1().touch())
    tmp = Weak1()
    ref2 = weakref.ref(tmp, lambda r: print("  weakref callback 2"))
    print(tmp.touch(), tmp.touch())
    tmp = None
    print("dead2:", ref2() is None)


run_weakrefs()

section("class mutation during argument evaluation")


class Late0:
    def f(self, x):
        return ("Late0.f", x)


class Late1(Late0):
    def f(self, x):
        return super().f(x)

    def swap_then_call(self):
        return super().f(install_replacement())


def install_replacement():
    Late0.f = lambda self, x: ("replacement", x)
    return "arg"


late = Late1()
print(late.f(1), late.f(2))
# CPython resolves `super().f` before evaluating the argument, so the call
# that installs the replacement still reaches the original method.
print(late.swap_then_call())
print(late.f(3), late.swap_then_call())
