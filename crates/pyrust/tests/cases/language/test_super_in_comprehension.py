# Zero-argument super() inside comprehensions (#3358).
#
# CPython 3.12 (PEP 709) inlines list/set/dict comprehensions into the
# enclosing function, so `super()` there sees the method's `__class__` cell and
# first parameter. Generator expressions are real frames: their first argument
# is the hidden `.0` iterator, so `super()` inside one raises TypeError.
import asyncio


def run(label, fn):
    try:
        print(label, fn())
    except BaseException as e:
        print(label, type(e).__name__, e)


class A:
    def m(self):
        return "A"

    @classmethod
    def cm(cls):
        return ("A", cls.__name__)

    @staticmethod
    def sm(x):
        return ("sm", x)

    attr = "attr-A"


class B(A):
    # --- the reproducer from the issue ---------------------------------
    def lc(self):
        return [super().m() for _ in range(2)]

    def sc(self):
        return sorted({super().m() for _ in range(2)})

    def dc(self):
        return {i: super().m() for i in range(2)}

    @classmethod
    def cm(cls):
        return [super().cm() for _ in range(2)]

    # --- comprehension shapes ------------------------------------------
    def nested(self):
        return [[super().m() for _ in range(2)] for _ in range(2)]

    def nested_dict_in_set_in_list(self):
        return [{k: [super().m() for _ in range(1)] for k in "ab"} for _ in range(1)]

    def multi_clause(self):
        return [super().m() for x in range(3) for y in range(2) if x != y]

    def in_condition(self):
        return [x for x in range(4) if super().m() == "A" and x % 2]

    def in_iterable(self):
        return [x for x in super().cm()]

    def in_dict_key_and_value(self):
        return {super().m(): super().m() for _ in range(2)}

    def attribute_not_call(self):
        return [super().attr for _ in range(2)]

    def bound_method(self):
        return [super().m for _ in range(1)][0]()

    def with_walrus(self):
        return [y := super().m() for _ in range(1)], y

    def explicit_two_arg(self):
        return [super(B, self).m() for _ in range(1)]

    def self_rebound(self):
        self = B()
        return [super().m() for _ in range(1)]

    def self_captured_by_closure(self):
        f = lambda: self
        return [super().m() for _ in range(1)], f() is self

    def direct_super_with_captured_self(self):
        f = lambda: self
        return super().m(), f() is self

    def comprehension_in_comprehension_iterable(self):
        return [x for x in [super().m() for _ in range(2)]]

    def comp_value_used(self):
        return [super().m() + str(self is not None) for _ in range(1)]

    # --- generator expressions are real frames: first arg is `.0` ------
    def genexp(self):
        return list(super().m() for _ in range(2))

    def listcomp_in_genexp(self):
        return list([super().m() for _ in range(1)] for _ in range(1))

    def genexp_in_listcomp(self):
        return [list(super().m() for _ in range(1)) for _ in range(1)]

    # --- nested scopes that are not inlined ----------------------------
    def lambda_no_arg(self):
        return [(lambda: super().m()) for _ in range(1)][0]()

    def lambda_with_arg(self):
        return [(lambda x: super().m()) for _ in range(1)][0](1)

    def nested_def_with_self_like_arg(self):
        def inner(me):
            return super().m()

        return [inner(self) for _ in range(1)]

    # --- generators / async methods ------------------------------------
    def gen_method(self):
        yield [super().m() for _ in range(2)]
        yield super().m()

    async def async_method(self):
        return [super().m() for _ in range(2)]

    async def async_comp(self):
        async def agen():
            yield 1
            yield 2

        return [(super().m(), i) async for i in agen()]


class P:
    def no_params():
        return super().x

    def only_varargs(*a):
        return super().x

    def kwonly(*, k):
        return super().x

    def kwonly_in_comp(*, k):
        return [super().m() for _ in range(1)]

    def comp_no_params():
        return [super().m() for _ in range(1)]

    def varargs_after_self(self, *a):
        return [super().m() for _ in range(1)]

    def class_cell_visible(self):
        return [__class__ for _ in range(1)]


def plain(self):
    return [super().m() for _ in range(1)]


def plain_genexp(self):
    return list(super().m() for _ in range(1))


def plain_no_args():
    return [super().m() for _ in range(1)]


b = B()
for name in [
    "lc",
    "sc",
    "dc",
    "nested",
    "nested_dict_in_set_in_list",
    "multi_clause",
    "in_condition",
    "in_iterable",
    "in_dict_key_and_value",
    "attribute_not_call",
    "bound_method",
    "with_walrus",
    "explicit_two_arg",
    "self_rebound",
    "self_captured_by_closure",
    "direct_super_with_captured_self",
    "comprehension_in_comprehension_iterable",
    "comp_value_used",
    "genexp",
    "listcomp_in_genexp",
    "genexp_in_listcomp",
    "lambda_no_arg",
    "lambda_with_arg",
    "nested_def_with_self_like_arg",
]:
    run(name, getattr(b, name))

run("cm", B.cm)
run("cm via instance", b.cm)
run("gen_method", lambda: list(b.gen_method()))
run("async_method", lambda: asyncio.run(b.async_method()))
run("async_comp", lambda: asyncio.run(b.async_comp()))

# Zero-parameter functions: RuntimeError, never TypeError.
run("P.no_params", P.no_params)
run("P.only_varargs", P().only_varargs)
run("P.kwonly", lambda: P.kwonly(k=1))
run("P.kwonly_in_comp", lambda: P.kwonly_in_comp(k=1))
run("P.comp_no_params", P.comp_no_params)
run("P.varargs_after_self", P().varargs_after_self)
run("P.class_cell_visible", P().class_cell_visible)


class C:
    def f():
        return super().x


run("C.f", C.f)

# Functions outside any class have no __class__ cell.
run("plain", lambda: plain(b))
run("plain_genexp", lambda: plain_genexp(b))
run("plain_no_args", plain_no_args)
run("module comp", lambda: [super().m() for _ in range(1)])
run("module scope", lambda: super())

# Behaviour through inheritance: the cell is the defining class, not type(self).
class Mixin:
    def who(self):
        return [super().who() for _ in range(1)]


class Base:
    def who(self):
        return "Base"


class Leaf(Mixin, Base):
    pass


run("mixin chain", Leaf().who)
