# Parity fixture for issue #3341: `__del__` finalizers run when the last
# reference to an instance goes away (rebinding, `del`, scope exit, container
# removal, discarded temporaries), on `gc.collect()`, and at interpreter exit.
#
# Every case prints from inside `__del__`, so the *position* of each line
# relative to the surrounding prints is what is compared against CPython.
# Exceptions raised by a finalizer are observed through `sys.unraisablehook`
# because the default hook prints an object address.

import gc
import sys
import weakref


class N:
    def __init__(self, n):
        self.n = n

    def __del__(self):
        print("del", self.n)


# ── The reproducer from the issue ────────────────────────────────────────────


def issue_reproducer():
    x = N(1)
    del x


issue_reproducer()
gc.collect()
print("done")

# ── Rebinding, expression statements, scope exit ─────────────────────────────

a = N("rebind-old")
a = N("rebind-new")
print("after rebind")
a = None
print("after rebind to None")

N("expression statement")
print("after expression statement")


def scope_exit():
    local = N("local")
    print("in scope_exit")


scope_exit()
print("after scope_exit")


def returns_instance():
    return N("returned")


returns_instance()
print("after discarded return value")
held = returns_instance()
print("holding returned value")
held = None
print("released returned value")


def two_locals():
    first = N("first local")
    second = N("second local")
    del second
    print("deleted second")


two_locals()
print("after two_locals")

# ── Aliases keep the instance alive ──────────────────────────────────────────

alias_a = N("aliased")
alias_b = alias_a
del alias_a
print("alias still alive")
del alias_b
print("alias gone")

# ── Call arguments and other temporaries ─────────────────────────────────────


def use(*args, **kwargs):
    return len(args) + len(kwargs)


use(N("arg"))
print("after call with temporary arg")


def use_two(first, second):
    return None


use_two(N("arg-a"), N("arg-b"))
print("after call with two temporary args")
print(use(N("arg-in-expr")) + 1)
print("after call inside expression")
use(key=N("kwarg"))
print("after call with temporary kwarg")
str(N("str-arg"))[:0]
print("after builtin call")
N("attr-read").n
print("after attribute read")

# ── Containers ───────────────────────────────────────────────────────────────

items = [N("list-0"), N("list-1")]
items.pop()
print("after list.pop")
del items[0]
print("after del list[0]")
items.append(N("list-2"))
items.clear()
print("after list.clear")
items = [N("list-3")]
items[0] = None
print("after list item store")

mapping = {"a": N("dict-a"), "b": N("dict-b")}
mapping.pop("a")
print("after dict.pop")
del mapping["b"]
print("after del dict[key]")
mapping["c"] = N("dict-c")
mapping["c"] = None
print("after dict item store")
mapping = {N("dict-key"): 1}
mapping.clear()
print("after dict.clear with instance key")

single = (N("tuple-0"),)
single = None
print("after tuple release")

members = {N("set-0")}
members.clear()
print("after set.clear")
members.add(N("set-1"))
members.pop()
print("after set.pop")

for index in range(2):
    N("loop-%d" % index)
print("after loop")

len([N("list-in-temp")])
print("after temporary list")


def container_alias():
    box = [N("boxed")]
    name = box[0]
    del name
    print("list still holds it")
    box.clear()
    print("list released it")


container_alias()

# ── Attributes ───────────────────────────────────────────────────────────────


class Holder:
    pass


holder = Holder()
holder.item = N("attr")
holder.item = None
print("after attribute store")
holder.item = N("attr-2")
del holder.item
print("after attribute delete")
holder.item = N("attr-3")
holder = None
print("after owner release")


def attach(owner):
    owner.item = N("attached by a callee")


# The owner exists before any finalizable object does, and is released by a
# frame other than the one that attached the finalizable member.
late_owner = Holder()
attach(late_owner)
print("after attach")
late_owner = None
print("after releasing an owner that predates its member")

# ── Ownership chains finalize owner first ────────────────────────────────────


class Node:
    def __init__(self, name, nxt=None):
        self.name = name
        self.nxt = nxt

    def __del__(self):
        print("node", self.name)


chain = Node("a", Node("b", Node("c")))
del chain
print("after chain")

# ── Inheritance, super(), dynamic installation and removal ───────────────────


class Base:
    def __del__(self):
        print("base del")


class Inherits(Base):
    pass


class Overrides(Base):
    def __del__(self):
        print("override del")
        super().__del__()


Inherits()
print("after inherited")
Overrides()
print("after override")


class Dynamic:
    pass


dynamic = Dynamic()
Dynamic.__del__ = lambda self: print("dynamic del")
del dynamic
print("after dynamic install")
dynamic = Dynamic()
del Dynamic.__del__
del dynamic
print("after dynamic removal")


class Slotted:
    __slots__ = ("value",)

    def __del__(self):
        print("slotted del")


Slotted()
print("after slotted")


class NoDel:
    pass


NoDel()
print("after class without __del__")



class FinalizedError(Exception):
    def __del__(self):
        print("exception del")


try:
    raise FinalizedError("raised")
except FinalizedError:
    print("handling exception with __del__")
print("after exception with __del__")

# ── Bound methods keep the receiver alive ────────────────────────────────────


class Method:
    def method(self):
        return "called"

    def __del__(self):
        print("method owner del")


bound = Method().method
print("holding bound method")
del bound
print("after bound method release")
print(Method().method())
print("after call on temporary")

# ── Finalizers that create and release finalizable objects ───────────────────


class Outer:
    def __del__(self):
        print("outer start")
        inner = N("inner")
        del inner
        N("inner temporary")
        print("outer end")


outer = Outer()
del outer
print("after nested finalizers")

# ── Finalizers that mutate the container being modified ──────────────────────

registry = []


class Registers:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        registry.append(self.name)


watched = [Registers("w0"), Registers("w1"), Registers("w2")]
watched.clear()
print("registry after clear:", sorted(registry))
table = {"k": Registers("in-dict")}
table.pop("k")
print("registry after pop:", sorted(registry))

# ── Resurrection: __del__ runs at most once (PEP 442) ────────────────────────

survivors = []


class Resurrects:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        print("resurrect del", self.name)
        survivors.append(self)


Resurrects("r")
print("survivors:", [s.name for s in survivors])
print("usable after resurrection:", survivors[0].name)
survivors.clear()
print("after second release")
gc.collect()
print("after collect")

# ── Identity and weak references inside __del__ ──────────────────────────────


class Identity:
    def __del__(self):
        print("same id:", id(self) == identity_id)
        print("weakref alive in __del__:", identity_ref() is self)


identity = Identity()
identity_id = id(identity)
identity_ref = weakref.ref(identity)
del identity
print("weakref dead afterwards:", identity_ref() is None)

# ── Exceptions raised by __del__ go to sys.unraisablehook ────────────────────


has_traceback = []


def hook(unraisable):
    print(
        "unraisable:",
        unraisable.exc_type.__name__,
        str(unraisable.exc_value),
        unraisable.err_msg,
        getattr(unraisable.object, "__qualname__", unraisable.object),
    )
    has_traceback.append(unraisable.exc_traceback is not None)


sys.unraisablehook = hook


class Raises:
    def __del__(self):
        raise ValueError("boom")


raises = Raises()
del raises
print("after raising __del__")
print("hook received a traceback:", has_traceback)
Raises()
print("after raising temporary")


class Divides:
    def __del__(self):
        1 / 0


divides = Divides()
divides = 5
print("after ZeroDivisionError in __del__")


def raising_scope():
    local = Raises()
    return "returned normally"


print(raising_scope())


class NotCallable:
    __del__ = 42


NotCallable()
print("after non-callable __del__")


# A finalizer's exception must not disturb the exception state around it.
try:
    doomed = Raises()
    del doomed
    raise KeyError("after finalizer")
except KeyError as exc:
    print("exception state intact:", repr(exc))
sys.unraisablehook = sys.__unraisablehook__

# ── A finalizer running while an exception propagates ────────────────────────


def raises_with_local():
    local = N("local of raising frame")
    raise LookupError("propagating")


try:
    raises_with_local()
except LookupError as exc:
    print("caught", exc)
print("after except block")


class Quiet:
    def __del__(self):
        try:
            raise TypeError("handled inside")
        except TypeError:
            print("handled inside __del__")


try:
    quiet = Quiet()
    del quiet
    raise IndexError("outer")
except IndexError as exc:
    print("outer exception survived:", exc)

# ── gc.collect() is safe to call repeatedly ──────────────────────────────────

collected = N("before collect")
del collected
print("collect returns int:", isinstance(gc.collect(), int))
print("collect again:", isinstance(gc.collect(), int))

# ── Interpreter exit: remaining globals are finalized in definition order ────

_private_at_exit = N("exit: _private (single underscore names go first)")
first_at_exit = N("exit: first")
second_at_exit = N("exit: second")
in_list_at_exit = [N("exit: inside a list")]
print("end of script")
