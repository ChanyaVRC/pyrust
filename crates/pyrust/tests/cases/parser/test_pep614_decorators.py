# Parity fixture for #3143: PEP 614 arbitrary decorator expressions
# (`'@' named_expression NEWLINE` in CPython 3.12).


def expect_syntax_error(src, label):
    try:
        compile(src, "<test>", "exec")
        raise AssertionError(f"expected SyntaxError for {label}: {src!r}")
    except SyntaxError:
        print("SyntaxError OK:", label)


# --- Lambda decorators (parenthesized and unparenthesized) ---
@(lambda f: lambda x: f(x) + 1)
def inc_wrapped(x):
    return x * 10


print("paren lambda:", inc_wrapped(3))


@lambda f: lambda x: f(x) * 2
def bare_lambda_wrapped(x):
    return x + 5


print("bare lambda:", bare_lambda_wrapped(3))


# --- Ternary / conditional decorator expressions ---
def add_one(f):
    return lambda x: f(x) + 1


def mul_ten(f):
    return lambda x: f(x) * 10


flag = True


@add_one if flag else mul_ten
def cond_true(x):
    return x


flag = False


@add_one if flag else mul_ten
def cond_false(x):
    return x


print("ternary:", cond_true(4), cond_false(4))


# --- Walrus (named expression) decorators ---
def make_adder(n):
    def dec(f):
        return lambda x: f(x) + n

    return dec


@(saved_dec := make_adder(7))
def walrus_paren(x):
    return x


@saved_dec2 := make_adder(100)
def walrus_bare(x):
    return x


print("walrus:", walrus_paren(3), walrus_bare(3), saved_dec(lambda x: x)(1), saved_dec2(lambda x: x)(1))


# --- Subscript, dict lookup, call, and boolean/binary expressions ---
decs = [add_one, mul_ten]
dec_map = {"a": add_one, "b": mul_ten}


@decs[0]
@dec_map["b"]
def subscript_fn(x):
    return x


print("subscript:", subscript_fn(2))


class Combiner:
    def __init__(self, tag):
        self.tag = tag

    def __add__(self, other):
        tag = f"{self.tag}+{other.tag}"

        def dec(f):
            return lambda: f"{tag}:{f()}"

        return dec


@Combiner("L") + Combiner("R")
def binop_fn():
    return "ok"


print("binop:", binop_fn())


@None or add_one
def boolop_fn(x):
    return x


print("boolop:", boolop_fn(9))


# --- Multi-line parenthesized decorator expression + blank line before def ---
@(
    add_one
    if True
    else mul_ten
)

def multiline_dec(x):
    return x


print("multiline:", multiline_dec(5))


# --- Evaluation & application order with stacked PEP 614 decorators ---
log = []


def record(name, inner):
    log.append(("eval", name))

    def dec(f):
        log.append(("apply", name))
        return inner(f)

    return dec


@record("first", add_one) if True else mul_ten
@(d_mid := record("second", mul_ten))
def stacked(x):
    return x


print("stacked:", stacked(2), log, callable(d_mid))


# --- Class decorators with PEP 614 expressions ---
def tag_cls(tag):
    def dec(cls):
        cls.tag = tag
        return cls

    return dec


@(cls_dec := tag_cls("walrus"))
@tag_cls("if_true") if True else tag_cls("if_false")
@lambda c: (setattr(c, "extra", 123) or c)
class Decorated:
    pass


print("class:", Decorated.tag, Decorated.extra, callable(cls_dec))


# --- Async def with PEP 614 decorator and `await` inside async def ---
async def get_dec():
    return lambda f: (lambda: ("wrapped", f))


async def make_async_decorated():
    @await get_dec()
    @(lambda f: f)
    async def inner():
        return 42

    return inner()


coro = make_async_decorated()
try:
    coro.send(None)
except StopIteration as stop:
    tag, inner_fn = stop.value
    inner_coro = inner_fn()
    try:
        inner_coro.send(None)
    except StopIteration as stop2:
        print("async dec:", tag, stop2.value)


# --- Parenthesized yield as decorator inside a generator ---
def gen_with_dec():
    @(yield "need_dec")
    def f():
        return 7

    return f()


g = gen_with_dec()
prompt = next(g)
try:
    g.send(lambda fn: lambda: fn() * 3)
except StopIteration as stop:
    print("gen yield dec:", prompt, stop.value)


# --- SyntaxError parity cases ---
expect_syntax_error("@d = 1\ndef f(): pass", "assignment in decorator")
expect_syntax_error("@dec def f(): pass", "missing newline before def")
expect_syntax_error("@dec; def f(): pass", "semicolon after decorator")
expect_syntax_error("@a, b\ndef f(): pass", "unparenthesized tuple decorator")
expect_syntax_error("@yield 1\ndef f(): pass", "bare yield decorator")
expect_syntax_error("@dec\nasync for x in a: pass", "decorated async for")
expect_syntax_error("@dec\nasync with cm: pass", "decorated async with")
expect_syntax_error("@dec\nx = 1", "decorated assignment")

print("pep614 decorators OK")
