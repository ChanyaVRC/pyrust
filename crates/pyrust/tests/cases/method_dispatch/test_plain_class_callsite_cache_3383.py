# Call-site construction cache for plain classes (issue #3383).
#
# A positional `Cls(...)` call site caches a validated construction plan for a
# plain class: default `type.__call__`, `object.__new__`, and either
# `object.__init__` or an ordinary-function `__init__`.  Every section below
# warms ONE call site (a helper function or a loop body) and then changes
# something the plan depends on; the next call through the same site must
# observe the change exactly as CPython does.  Output must be byte-identical
# to python3.12.

import abc
import sys
from dataclasses import dataclass


def section(title):
    print(f"--- {title}")


def attempt(thunk):
    try:
        return thunk()
    except Exception as exc:  # noqa: BLE001 - the message is the assertion
        return f"{type(exc).__name__}: {exc}"


# ---------------------------------------------------------------- 1. basics
section("basics")


class Empty:
    pass


class Pair:
    def __init__(self, a, b):
        self.a = a
        self.b = b


class Slots2:
    __slots__ = ("a", "b")

    def __init__(self, a, b):
        self.a = a
        self.b = b


def make_empty():
    return Empty()


def make_pair(i):
    return Pair(i, i + 1)


def make_slots(i):
    return Slots2(i, i * 2)


empties = [make_empty() for _ in range(5)]
print(len({id(e) for e in empties}), all(type(e) is Empty for e in empties))
print(all(e.__dict__ == {} for e in empties))
pairs = [make_pair(i) for i in range(5)]
print([(p.a, p.b) for p in pairs], len({id(p) for p in pairs}))
slotted = [make_slots(i) for i in range(5)]
print([(s.a, s.b) for s in slotted], hasattr(slotted[0], "__dict__"))
print(attempt(lambda: setattr(slotted[0], "c", 1)))

# ------------------------------------------- 2. object.__init__ and arguments
section("no __init__ rejects arguments")


def make_empty_arg():
    return Empty(1)


def make_empty_args():
    return Empty(1, 2, 3)


for _ in range(3):
    print(attempt(make_empty_arg))
    print(attempt(make_empty_args))
print(type(make_empty()).__name__)

# ------------------------------------------------------ 3. __init__ arity
section("__init__ arity and defaults")


class Defaults:
    def __init__(self, a, b=5, *rest, key="k"):
        self.state = (a, b, rest, key)


class KwOnly:
    def __init__(self, a, *, flag=False):
        self.state = (a, flag)


def pair_one():
    return Pair(1)


def pair_three():
    return Pair(1, 2, 3)


def pair_none():
    return Pair()


for _ in range(3):
    print(attempt(pair_one))
    print(attempt(pair_three))
    print(attempt(pair_none))
for _ in range(3):
    print(Defaults(1).state, Defaults(1, 2).state, Defaults(1, 2, 3, 4).state)
    print(KwOnly(7).state, attempt(lambda: KwOnly(7, True)))
    print(attempt(lambda: Defaults()))



class PlainDefaults:
    def __init__(self, a, b=5, c=None, shared=[]):
        shared.append(a)
        self.state = (a, b, c, len(shared))


def defaults_one(a):
    return PlainDefaults(a)


def defaults_two(a, b):
    return PlainDefaults(a, b)


def defaults_all(a, b, c, shared):
    return PlainDefaults(a, b, c, shared)


def defaults_none():
    return PlainDefaults()


def defaults_too_many():
    return PlainDefaults(1, 2, 3, [], 5)


for i in range(3):
    print(defaults_one(i).state, defaults_two(i, i).state, defaults_all(i, i, i, []).state)
    print(attempt(defaults_none))
    print(attempt(defaults_too_many))
PlainDefaults.__init__.__defaults__ = ("B", "C", [])
print(defaults_one(1).state, defaults_two(1, 2).state)
PlainDefaults.__init__.__defaults__ = ([],)
print(attempt(lambda: defaults_one(1)))
print(attempt(lambda: defaults_two(1, 2)))
print(defaults_all(1, 2, 3, []).state, PlainDefaults(1, 2, 3).state)
PlainDefaults.__init__.__defaults__ = None
print(attempt(lambda: defaults_all(1, 2, 3, []).state))
print(attempt(lambda: PlainDefaults(1, 2, 3)))

# ------------------------------------------ 4. __init__ must return None
section("__init__ return value")


class Returns:
    value = None

    def __init__(self):
        self.ready = True
        return Returns.value


def make_returns():
    return Returns()


print(make_returns().ready, make_returns().ready)
for value in (1, "text", [], None, 0, (), None):
    Returns.value = value
    result = attempt(make_returns)
    print(result if isinstance(result, str) else result.ready)


class GeneratorInit:
    def __init__(self):
        yield 1


for _ in range(3):
    print(attempt(lambda: GeneratorInit()))

# ------------------------------------------ 5. __init__ raising / re-entrancy
section("__init__ raising and re-entrancy")


class Raises:
    def __init__(self, n):
        if n % 2:
            raise ValueError(f"odd {n}")
        self.n = n


def make_raises(n):
    return Raises(n)


for n in range(5):
    result = attempt(lambda: make_raises(n))
    print(result if isinstance(result, str) else result.n)


class Node:
    def __init__(self, depth):
        self.depth = depth
        self.child = Node(depth - 1) if depth else None


def chain_length(node):
    count = 0
    while node is not None:
        count += 1
        node = node.child
    return count


print([chain_length(Node(depth)) for depth in (0, 1, 5, 20)])


class Closure:
    def __init__(self, x, y):
        self.get = lambda: (x, y, self.tag)
        self.tag = "t"


print([Closure(i, -i).get() for i in range(3)])

# -------------------------------------------- 6. replacing / deleting __init__
section("dynamic __init__")


class Dyn:
    def __init__(self, x):
        self.x = x


def make_dyn(x):
    return Dyn(x)


def make_dyn_bare():
    return Dyn()


print([make_dyn(i).x for i in range(3)])


def doubled(self, x):
    self.x = x * 2


Dyn.__init__ = doubled
print([make_dyn(i).x for i in range(3)])
Dyn.__init__ = staticmethod(lambda *args: print("static init", args))
print(make_dyn(4).__dict__)
Dyn.__init__ = classmethod(lambda cls, *args: print("class init", cls.__name__, args))
print(make_dyn(5).__dict__)
del Dyn.__init__
print(attempt(lambda: make_dyn(6)))
print(make_dyn_bare().__dict__)
Dyn.__init__ = lambda self: setattr(self, "x", "fresh")
print(make_dyn_bare().x)
print(attempt(lambda: make_dyn(7)))


class InitBase:
    def __init__(self, v):
        self.v = ("base", v)


class InitChild(InitBase):
    pass


class InitGrandChild(InitChild):
    pass


def make_grandchild(v):
    return InitGrandChild(v)


print([make_grandchild(i).v for i in range(2)])
InitBase.__init__ = lambda self, v: setattr(self, "v", ("patched base", v))
print([make_grandchild(i).v for i in range(2)])
InitChild.__init__ = lambda self, v: setattr(self, "v", ("child", v))
print([make_grandchild(i).v for i in range(2)])
del InitChild.__init__
print([make_grandchild(i).v for i in range(2)])
del InitBase.__init__
print(attempt(lambda: make_grandchild(1)))


class SelfPatching:
    def __init__(self):
        self.kind = "first"
        SelfPatching.__init__ = SelfPatching.second

    def second(self):
        self.kind = "second"


def make_self_patching():
    return SelfPatching()


print([make_self_patching().kind for _ in range(4)])

# -------------------------------------------------------- 7. custom __new__
section("dynamic __new__")


class NewTarget:
    def __init__(self, x):
        self.x = x


def make_new_target(x):
    return NewTarget(x)


print([make_new_target(i).x for i in range(3)])


def tagging_new(cls, x):
    obj = object.__new__(cls)
    obj.tag = "tagged"
    return obj


NewTarget.__new__ = tagging_new
made = make_new_target(3)
print(made.x, made.tag)
# `__new__` returning a foreign object skips `__init__` (issue #1143).
NewTarget.__new__ = lambda cls, x: ["foreign", x]
print(make_new_target(4), make_new_target(5))
del NewTarget.__new__
print(attempt(lambda: make_new_target(6).x))


class NewFromStart:
    def __new__(cls, x):
        obj = super().__new__(cls)
        obj.origin = "new"
        return obj

    def __init__(self, x):
        self.x = x


class NewForeign:
    def __new__(cls):
        return 42

    def __init__(self):
        raise AssertionError("__init__ must not run for a foreign object")


for i in range(3):
    made = NewFromStart(i)
    print(made.x, made.origin, NewForeign())


class NewOnBase:
    pass


class NewOnChild(NewOnBase):
    pass


def make_new_on_child():
    return NewOnChild()


print(type(make_new_on_child()).__name__, type(make_new_on_child()).__name__)
NewOnBase.__new__ = lambda cls: "base new"
print(make_new_on_child(), make_new_on_child())

# ------------------------------------------------------------ 8. metaclasses
section("metaclass __call__")


class Meta(type):
    pass


class WithMeta(metaclass=Meta):
    def __init__(self, x=0):
        self.x = x


def make_with_meta():
    return WithMeta(3)


print([make_with_meta().x for _ in range(3)])
Meta.__call__ = lambda cls, *args: ("intercepted", cls.__name__, args)
print(make_with_meta(), make_with_meta())


def counting_call(cls, *args):
    obj = type.__call__(cls, *args)
    obj.via_meta = True
    return obj


Meta.__call__ = counting_call
made = make_with_meta()
print(made.x, made.via_meta)
del Meta.__call__
made = make_with_meta()
print(made.x, hasattr(made, "via_meta"))


class MetaBase(type):
    pass


class MetaDerived(MetaBase):
    pass


class DeepMeta(metaclass=MetaDerived):
    pass


def make_deep_meta():
    return DeepMeta()


print(type(make_deep_meta()).__name__, type(make_deep_meta()).__name__)
MetaBase.__call__ = lambda cls: "base metaclass call"
print(make_deep_meta(), make_deep_meta())
del MetaBase.__call__
print(type(make_deep_meta()).__name__)


class CallFromStart(type):
    def __call__(cls, *args):
        obj = super().__call__(*args)
        obj.wrapped = True
        return obj


class StaticCall(type):
    __call__ = staticmethod(lambda *args: ("static", args))


class ClassCall(type):
    @classmethod
    def __call__(mcs, *args):
        return ("classmethod", mcs.__name__, args)


class Wrapped(metaclass=CallFromStart):
    pass


class Static(metaclass=StaticCall):
    pass


class ViaClass(metaclass=ClassCall):
    pass


for _ in range(3):
    print(Wrapped().wrapped, Static(), ViaClass())

# ------------------------------------------------------- 9. abstract classes
section("abstract classes")


class Shape(abc.ABC):
    @abc.abstractmethod
    def area(self):
        ...


class Square(Shape):
    def __init__(self, side):
        self.side = side

    def area(self):
        return self.side**2


def make_shape():
    return Shape()


def make_square(side):
    return Square(side)


for _ in range(3):
    print(attempt(make_shape))
print([make_square(i).area() for i in range(3)])
Square.__abstractmethods__ = frozenset({"perimeter"})
print(attempt(lambda: make_square(2)))
Square.__abstractmethods__ = frozenset()
print(make_square(3).area())
Shape.__abstractmethods__ = frozenset()
print(type(make_shape()).__name__)
Shape.__abstractmethods__ = frozenset({"area"})
print(attempt(make_shape))


class Late(abc.ABC):
    def __init__(self):
        self.ok = True


def make_late():
    return Late()


print(make_late().ok, make_late().ok)
Late.run = abc.abstractmethod(lambda self: None)
abc.update_abstractmethods(Late)
print(attempt(make_late))
del Late.run
abc.update_abstractmethods(Late)
print(make_late().ok)

# ------------------------------------------------- 10. built-in subclasses
section("built-in subclasses")


class MyInt(int):
    pass


class MyList(list):
    pass


class MyDict(dict):
    pass


class MyStr(str):
    pass


class MyTuple(tuple):
    pass


class MyExc(Exception):
    pass


class MyExcInit(Exception):
    def __init__(self, code):
        super().__init__(f"code {code}")
        self.code = code


for i in range(3):
    print(
        MyInt(i) + 1,
        MyList([i]) + [0],
        MyDict(k=i),
        MyStr(i).zfill(3),
        MyTuple((i, i)),
        repr(MyExc(i)),
        MyExcInit(i).args,
        MyExcInit(i).code,
    )
    print(MyInt(), MyList(), MyDict(), repr(MyStr()), MyTuple(), repr(MyExc()))

# ------------------------------------------------ 11. polymorphic call sites
section("polymorphic call sites")


class PolyA:
    def __init__(self):
        self.name = "a"


class PolyB:
    def __init__(self):
        self.name = "b"


def build(factory):
    return factory()


print([build(cls).name for cls in (PolyA, PolyB, PolyA, PolyB, PolyB, PolyA)])
print(build(int), build(list), build(lambda: "fn"), build(PolyA).name, build(dict))
print(build(Empty).__class__.__name__, build(str) == "", build(PolyB).name)


def local_class(n):
    class Local:
        def __init__(self, value):
            self.value = value * n

    return Local(10)


print([local_class(n).value for n in range(4)])
print(len({type(local_class(n)) for n in range(4)}))

# ------------------------------------------------ 12. unrelated mutations
section("unrelated class mutations")


class Counted:
    count = 0

    def __init__(self, tag):
        Counted.count += 1
        self.tag = tag


class Bystander:
    def __init__(self, n):
        self.n = n


def make_counted(tag):
    return Counted(tag)


def make_bystander(n):
    return Bystander(n)


total = 0
for i in range(50):
    total += make_bystander(i).n
    make_counted(i)
print(total, Counted.count)

# ------------------------------------------------------------ 13. finalizers
section("finalizers")


class Finalized:
    def __init__(self, n):
        self.n = n

    def __del__(self):
        print("del Finalized", self.n)


def make_finalized(n):
    return Finalized(n)


for i in range(4):
    obj = make_finalized(i)
    del obj
    print("after", i)


class BareFinalized:
    def __del__(self):
        print("del BareFinalized")


def make_bare_finalized():
    return BareFinalized()


for i in range(3):
    obj = make_bare_finalized()
    del obj
    print("after bare", i)


class LateFinalizer:
    def __init__(self, n):
        self.n = n


def make_late_finalizer(n):
    return LateFinalizer(n)


# Instances made before `__del__` exists stay alive (and silent: n < 100), so
# only the ones constructed through the already-warm site are observed.
warm_instances = [make_late_finalizer(i) for i in range(3)]
print("warmed without __del__", len(warm_instances))


def late_del(self):
    if self.n >= 100:
        print("del LateFinalizer", self.n)


def replaced_del(self):
    if self.n >= 100:
        print("replaced __del__", self.n)


LateFinalizer.__del__ = late_del
for i in range(100, 103):
    obj = make_late_finalizer(i)
    del obj
    print("after late", i)
LateFinalizer.__del__ = replaced_del
obj = make_late_finalizer(109)
del obj
del LateFinalizer.__del__
obj = make_late_finalizer(110)
del obj
print("finalizer removed")


class FinalizerBase:
    def __del__(self):
        print("del via base", type(self).__name__)


class FinalizerLeft:
    def __init__(self):
        self.left = True


class FinalizerDiamond(FinalizerLeft, FinalizerBase):
    pass


def make_diamond():
    return FinalizerDiamond()


for i in range(3):
    obj = make_diamond()
    print(obj.left)
    del obj
    print("after diamond", i)

# --------------------------------------------- 14. multiple inheritance
section("multiple inheritance")


class Left:
    def __init__(self, *args):
        self.order = ["left"]
        super().__init__(*args)


class Right:
    def __init__(self, tag):
        self.order.append(f"right {tag}")


class Both(Left, Right):
    pass


def make_both(tag):
    return Both(tag)


print([make_both(i).order for i in range(3)])
Right.__init__ = lambda self, tag: self.order.append(f"patched {tag}")
print(make_both(7).order)

# ------------------------------------------------- 15. non-function __init__
section("non-function __init__")


class CallableInit:
    def __call__(self, *args):
        print("callable init", len(args))


class PropertyInit:
    @property
    def __init__(self):
        return lambda *args: print("property init", args)


class ObjectInit:
    __init__ = CallableInit()


class IntInit:
    __init__ = 5


for _ in range(2):
    print(type(PropertyInit(1)).__name__)
    print(type(ObjectInit(1, 2)).__name__)
    print(attempt(lambda: IntInit()))

# ------------------------------------------------------- 16. introspection
section("frames and dataclasses")


class FrameProbe:
    def __init__(self):
        caller = sys._getframe(1)
        self.caller_name = caller.f_code.co_name
        self.caller_line = caller.f_lineno


def probe_caller():
    made = FrameProbe(); line = sys._getframe().f_lineno  # noqa: E702
    return made.caller_name, made.caller_line == line


print([probe_caller() for _ in range(3)])


@dataclass
class Point:
    x: int
    y: int = 0


@dataclass(slots=True)
class SlotPoint:
    x: int
    y: int


print([Point(i) for i in range(2)], [Point(i, i) for i in range(2)])
print([SlotPoint(i, -i) for i in range(2)], attempt(lambda: Point(1, 2, 3)) != "")
print(Point(1, 2) == Point(1, 2), Point(1) != Point(2))

profiled = []


def profiler(frame, event, arg):
    if event in ("call", "return") and frame.f_code.co_name == "__init__":
        profiled.append(event)


sys.setprofile(profiler)
for i in range(3):
    make_pair(i)
sys.setprofile(None)
print(profiled)
