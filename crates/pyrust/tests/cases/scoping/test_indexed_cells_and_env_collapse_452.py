import sys

# 1. Multi-level transitive pass-through captures (middle scopes do not touch x/y)
def level0():
    z = 100
    a = 1

    def level1():
        def level2():
            def level3():
                nonlocal a
                a += 5
                return (z, a)

            return level3

        return level2

    l2 = level1()
    l3 = l2()
    print("transitive co_cellvars:", level0.__code__.co_cellvars)
    print("transitive l1 co_freevars:", level1.__code__.co_freevars)
    print("transitive l2 co_freevars:", l2.__code__.co_freevars)
    print("transitive l3 co_freevars:", l3.__code__.co_freevars)
    print("transitive call 1:", l3())
    print("transitive call 2:", l3())
    print("transitive outer:", (z, a))


level0()


# 2. Sibling closures sharing mutable cells, del + rebind, and all write forms
def make_counter_family():
    count = 0

    def inc(n=1):
        nonlocal count
        count += n
        return count

    def dec(n=1):
        nonlocal count
        count -= n
        return count

    def get():
        return count

    def clear():
        nonlocal count
        del count

    def set_via_forms():
        nonlocal count
        for count in [10, 20]:
            pass
        assert count == 20
        v = (count := count + 3)
        assert v == 23 and count == 23
        count, _ = (30, 0)
        assert count == 30

        class Ctx:
            def __enter__(self):
                return 42

            def __exit__(self, *args):
                return False

        with Ctx() as count:
            pass
        return count

    return inc, dec, get, clear, set_via_forms


inc, dec, get, clear, set_via_forms = make_counter_family()
print("siblings:", inc(5), dec(2), get(), set_via_forms(), get())
clear()
try:
    get()
except NameError as exc:
    print("deleted freevar error:", type(exc).__name__, str(exc))
inc_val = set_via_forms()
print("rebound after delete:", inc_val, get())


def owner_unbound_after_delete():
    v = 7

    def reader():
        return v

    def deleter():
        nonlocal v
        del v

    assert reader() == 7
    deleter()
    try:
        return v
    except UnboundLocalError as exc:
        print("owner unbound error:", type(exc).__name__, str(exc))


owner_unbound_after_delete()


# 3. Live cell objects via __closure__: get/set/del, empty cell ValueError, is/==
def cell_introspection():
    def read_early():
        return x

    c_early = read_early.__closure__[0]
    print("empty cell repr has empty:", "empty" in repr(c_early))
    try:
        _ = c_early.cell_contents
    except ValueError as exc:
        print("empty cell read:", type(exc).__name__, str(exc))
    try:
        hasattr(c_early, "cell_contents")
    except ValueError as exc:
        print("empty cell hasattr:", type(exc).__name__, str(exc))

    x = 123

    def read_late():
        return x

    c_late = read_late.__closure__[0]
    print("same cell identity:", c_early is c_late)
    print("same cell eq:", c_early == c_late)
    print("bound cell read:", c_early.cell_contents, read_early(), read_late())
    print("bound cell repr has int:", "int object" in repr(c_early))

    c_early.cell_contents = 456
    print("live cell write observed:", x, read_early(), read_late())

    del c_early.cell_contents
    del c_early.cell_contents  # idempotent on empty cell in CPython 3.12
    try:
        read_late()
    except NameError as exc:
        print("after del cell_contents:", type(exc).__name__)

    c_late.cell_contents = 789
    print("after re-set cell_contents:", x, read_early())

    # Distinct outer invocations produce distinct cell identities with equal contents
    def make_box(val):
        def f():
            return val

        return f.__closure__[0]

    b1 = make_box(10)
    b2 = make_box(10)
    b3 = make_box(20)
    print("distinct cell identity/eq:", b1 is b2, b1 == b2, b1 == b3)
    del b1.cell_contents
    del b2.cell_contents
    print("empty cell eq:", b1 == b2, b1 == b3)


cell_introspection()


# 4. Class body LOAD_CLASSDEREF, nonlocal in class body (#735), and super()
GLOBAL_VAR = "module_global"


def class_deref_tests():
    captured = "from_enclosing"
    mutable = 1

    class WithDeref:
        seen = captured
        nonlocal mutable
        mutable = 99

        def method(self):
            return (captured, mutable, __class__.__name__)

    print("class deref & nonlocal:", WithDeref.seen, mutable, WithDeref().method())

    class WithLocalsOverride:
        locals()["captured"] = "from_class_dict"
        seen = captured

    print("class locals override:", WithLocalsOverride.seen)

    # Unbound enclosing cell in class body raises NameError (must not fall back to module global)
    GLOBAL_VAR_LOCAL = None
    del GLOBAL_VAR_LOCAL

    class UnboundInClass:
        try:
            _ = GLOBAL_VAR_LOCAL
        except NameError as exc:
            err = (type(exc).__name__, "GLOBAL_VAR_LOCAL" in str(exc))

    print("class unbound deref:", UnboundInClass.err)

    # Zero-arg super() + enclosing cell capture
    class Base:
        def greet(self):
            return "base"

    class Child(Base):
        def greet(self):
            return super().greet() + ":" + captured

    print("class super + cell:", Child().greet())


class_deref_tests()


# 5. Generators & coroutines owning and closing over cells
def generator_cell_tests():
    total = 0

    def gen(start):
        state = start

        def bump(delta):
            nonlocal state
            state += delta

        yield (state, bump, sorted(locals().keys()))
        nonlocal total
        total += state
        yield (state, total, sys._getframe().f_locals["state"])

    g = gen(10)
    s1, bump, loc_keys = next(g)
    print("gen locals keys:", loc_keys)
    bump(7)
    s2, t2, frame_state = next(g)
    print("gen after external cell bump:", s1, s2, t2, frame_state, total)


generator_cell_tests()


def coroutine_cell_tests():
    acc = []

    async def coro(val):
        nonlocal acc
        acc.append(val)
        return len(acc)

    c = coro("hi")
    try:
        c.send(None)
    except StopIteration as stop:
        print("coro result:", stop.value, acc)


coroutine_cell_tests()


# 6. PEP 709 inlined comprehensions with cells and unbound error kinds
def comprehension_cell_tests():
    factor = 3
    last = 0
    out = [(last := i * factor) for i in range(4)]

    def peek():
        return (factor, last)

    print("comp walrus into cell:", out, peek())

    def inner_unbound():
        try:
            _ = [missing_local for _ in range(1)]
        except UnboundLocalError as exc:
            print("comp own unbound:", type(exc).__name__)
        missing_local = 1
        return missing_local

    inner_unbound()

    def outer_missing():
        def inner_free():
            try:
                _ = [missing_free for _ in range(1)]
            except NameError as exc:
                print("comp free unbound:", type(exc).__name__)

        inner_free()
        missing_free = 1
        return missing_free

    outer_missing()


comprehension_cell_tests()


# 7. `global` inside non-root defining envs (class method + PEP 695 generic)
METHOD_GLOBAL = "initial"
T = "module_T"


class GlobalInMethod:
    METHOD_GLOBAL = "class_attr"

    def set_global(self, val):
        global METHOD_GLOBAL
        METHOD_GLOBAL = val
        return METHOD_GLOBAL


gim = GlobalInMethod()
print(
    "global in method:",
    gim.set_global("updated"),
    METHOD_GLOBAL,
    GlobalInMethod.METHOD_GLOBAL,
)


def generic_with_global_T():
    global T
    before = T
    T = "mutated_module_T"

    def identity[T](x: T) -> T:
        return x

    after = T
    return (before, after, identity(99), identity.__type_params__[0].__name__)


print("pep695 + global T:", generic_with_global_T(), T)


# 8. `except E as e:` when `e` is a cell variable (PEP 3110 cleanup)
def except_cell_cleanup():
    captured_fn = None
    try:
        raise RuntimeError("boom")
    except RuntimeError as e:
        def get_e():
            return str(e)

        captured_fn = get_e
        print("inside except:", captured_fn())

    try:
        captured_fn()
    except NameError as exc:
        print("after except cleanup:", type(exc).__name__)


except_cell_cleanup()


# 9. co_cellvars, co_freevars, locals(), f_locals with dead-branch & unused nonlocal
def introspection_tests():
    z = 1
    a = 2
    unused_outer = 3
    unbound_cell = 4
    del unbound_cell

    def inner():
        nonlocal unused_outer
        if False:
            return z
        frame_locals = sys._getframe().f_locals
        return (a, sorted(locals().keys()), sorted(frame_locals.keys()))

    print("outer locals keys:", sorted(k for k in locals().keys() if k != "inner"))
    print("outer co_cellvars:", introspection_tests.__code__.co_cellvars)
    print("inner co_freevars:", inner.__code__.co_freevars)
    print("inner locals/f_locals:", inner())


introspection_tests()


# 10. __del__ finalizer when deleting cell / nonlocal binding
del_log = []


class Finalizer:
    def __init__(self, tag):
        self.tag = tag

    def __del__(self):
        del_log.append(self.tag)


def cell_del_finalizer():
    obj = Finalizer("a")
    alias = obj

    def drop_obj():
        nonlocal obj
        del obj

    def drop_alias():
        nonlocal alias
        del alias

    return drop_obj, drop_alias


d1, d2 = cell_del_finalizer()
d1()
print("del_log after drop_obj:", del_log)
d2()
print("del_log after drop_alias:", del_log)
