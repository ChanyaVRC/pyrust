# Parenthesised `(yield)` / `(yield x)` / `(yield from xs)` inside a `lambda`
# body (#3390): the lambda is compiled as a generator function (CO_GENERATOR),
# `send` / `throw` / `close` work, and `ast.parse` / `ast.dump` / `ast.unparse`
# agree with CPython 3.12.3.  An unparenthesised `yield` in a lambda body is a
# SyntaxError.

import ast
import inspect

CO_GENERATOR = 32


def show(label, value):
    print(label, value)


# --- 1. Reproducer from the issue ---

f = lambda: (yield)
g = lambda x: (yield x) + 10
h = lambda: (yield from [1, 2, 3])
show("f flag", bool(f.__code__.co_flags & CO_GENERATOR))
show("g flag", bool(g.__code__.co_flags & CO_GENERATOR))
show("h flag", bool(h.__code__.co_flags & CO_GENERATOR))
it = g(5)
print(next(it), end=" ")
try:
    it.send(7)
except StopIteration as e:
    print(e.value)
show("h list", list(h()))


# --- 2. Introspection: yield-lambdas are generator functions ---

show("isgeneratorfunction f", inspect.isgeneratorfunction(f))
show("isgeneratorfunction g", inspect.isgeneratorfunction(g))
show("isgeneratorfunction plain", inspect.isgeneratorfunction(lambda: 1))
show("plain flag", bool((lambda: 1).__code__.co_flags & CO_GENERATOR))
gen = f()
show("isgenerator", inspect.isgenerator(gen))
show("gen name", gen.__name__)
show("gen qualname", gen.__qualname__.endswith("<lambda>"))
show("code name", f.__code__.co_name)
show("repr type", type(gen).__name__)
show("nested lambda not generator", bool((lambda: (lambda: 1)).__code__.co_flags & CO_GENERATOR))

outer = lambda: (lambda: (yield))
show("outer flag", bool(outer.__code__.co_flags & CO_GENERATOR))
show("inner flag", bool(outer().__code__.co_flags & CO_GENERATOR))


# --- 3. Bare yield returns the sent value to the surrounding expression ---

echo = lambda: (yield)
gen = echo()
show("echo next", next(gen))
try:
    gen.send("sent")
except StopIteration as e:
    show("echo return", e.value)

twice = lambda: (yield) + (yield)
gen = twice()
next(gen)
gen.send(3)
try:
    gen.send(4)
except StopIteration as e:
    show("twice return", e.value)

plus = lambda x: (yield x) + 10
gen = plus(1)
show("plus first", next(gen))
try:
    gen.send(5)
except StopIteration as e:
    show("plus return", e.value)

# next() sends None
gen = plus(1)
next(gen)
try:
    next(gen)
except TypeError as e:
    show("none + 10", type(e).__name__)


# --- 4. Yielded values: tuples, conditional, subexpressions ---

tup = lambda: (yield 1, 2)
show("yield tuple", next(tup()))
star = lambda a: (yield *a, 9)
show("yield star", next(star([7, 8])))
cond = lambda c: (yield 1 if c else 2)
show("yield cond true", next(cond(True)))
show("yield cond false", next(cond(False)))
nt = lambda x: (yield not x)
show("yield not", next(nt(0)))
yl = lambda: (yield lambda: 5)
show("yield lambda", next(yl())())
yy = lambda: (yield (yield 1))
gen = yy()
show("nested yield 1", next(gen))
show("nested yield 2", gen.send("a"))
try:
    gen.send("b")
except StopIteration as e:
    show("nested ret", e.value)


# --- 5. yield from delegation inside lambdas ---

yf = lambda xs: (yield from xs)
show("yield from list", list(yf([1, 2])))
show("yield from range", list(yf(range(3))))
show("yield from gen", list(yf(x * x for x in range(4))))
show("yield from empty", list(yf([])))


def sub():
    got = yield "ready"
    return "sub:" + str(got)


yfr = lambda: (yield from sub())
gen = yfr()
show("yf first", next(gen))
try:
    gen.send("v")
except StopIteration as e:
    show("yf value", e.value)

yfv = lambda: ((yield from sub()), "tail")
gen = yfv()
next(gen)
try:
    gen.send("w")
except StopIteration as e:
    show("yf tuple value", e.value)

yfs = lambda it: (yield from it)
gen = yfs(iter([10, 20]))
show("yf send 1", next(gen))
show("yf send 2", gen.send(None))


# --- 6. throw() / close() ---

thr = lambda: (yield 1)
gen = thr()
next(gen)
try:
    gen.throw(ValueError("boom"))
except ValueError as e:
    show("throw propagates", e.args)
show("after throw", inspect.getgeneratorstate(gen))

gen = thr()
try:
    gen.throw(KeyError("pre"))
except KeyError as e:
    show("throw before start", e.args)

gen = thr()
next(gen)
show("close result", gen.close())
show("closed state", inspect.getgeneratorstate(gen))
try:
    next(gen)
except StopIteration:
    show("closed next", "StopIteration")

gen = thr()
show("fresh state", inspect.getgeneratorstate(gen))
next(gen)
show("suspended state", inspect.getgeneratorstate(gen))
try:
    gen.send(None)
except StopIteration as e:
    show("completed value", e.value)
show("completed state", inspect.getgeneratorstate(gen))

# throw into a yield from delegate
thrf = lambda it: (yield from it)


def catcher():
    try:
        yield "a"
    except ValueError:
        yield "caught"
    return "end"


gen = thrf(catcher())
show("deleg next", next(gen))
show("deleg throw", gen.throw(ValueError()))

# send to a just-started generator
gen = f()
try:
    gen.send(1)
except TypeError as e:
    show("send non-None to fresh", type(e).__name__)


# --- 7. Arguments, defaults, closures, scoping ---

args = lambda a, b=2, *c, d, **e: (yield a, b, c, d, e)
show("args", next(args(1, d=4, z=5)))

base = 100
clo = lambda n: (yield base + n)
show("closure", next(clo(1)))
base = 200
show("closure rebound", next(clo(1)))


def make(k):
    return lambda: (yield k)


show("factory", [next(make(i)()) for i in range(3)])

cls_body = type("C", (), {"m": lambda self: (yield self.v), "v": 11})
show("method", next(cls_body().m()))


class K:
    gen = lambda self: (yield from range(2))


show("class lambda", list(K().gen()))

walrus = lambda: (x := (yield))
gen = walrus()
next(gen)
try:
    gen.send(9)
except StopIteration as e:
    show("walrus value", e.value)

attr = lambda: (yield).real
gen = attr()
next(gen)
try:
    gen.send(5)
except StopIteration as e:
    show("attr value", e.value)

sub_ = lambda: (yield)[0]
gen = sub_()
next(gen)
try:
    gen.send([42])
except StopIteration as e:
    show("subscript value", e.value)

call = lambda: (yield)(3)
gen = call()
next(gen)
try:
    gen.send(lambda n: n * 2)
except StopIteration as e:
    show("call value", e.value)

neg = lambda: -(yield)
gen = neg()
next(gen)
try:
    gen.send(4)
except StopIteration as e:
    show("neg value", e.value)

boolop = lambda: (yield) or "fallback"
gen = boolop()
next(gen)
try:
    gen.send(0)
except StopIteration as e:
    show("or value", e.value)

lst = lambda: [(yield), (yield)]
gen = lst()
next(gen)
gen.send("a")
try:
    gen.send("b")
except StopIteration as e:
    show("list value", e.value)

dct = lambda: {(yield): (yield)}
gen = dct()
next(gen)
gen.send("k")
try:
    gen.send("v")
except StopIteration as e:
    show("dict value", e.value)

fstr = lambda: f"<{(yield)}>"
gen = fstr()
next(gen)
try:
    gen.send("x")
except StopIteration as e:
    show("fstring value", e.value)

ternary = lambda c: (yield "t") if c else "no"
show("ternary yield", next(ternary(True)))
try:
    next(ternary(False))
except StopIteration as e:
    show("ternary skip", e.value)

doubled = ((lambda: (yield))())
show("doubled paren", next(doubled))
paren2 = lambda: ((yield))
show("double paren", next(paren2()))


# --- 8. Lambda bodies as iterables / in comprehensions of lambdas ---

lams = [lambda i=i: (yield i) for i in range(3)]
show("lambda list", [next(l()) for l in lams])
show("lambda as map", [list(r) for r in map(lambda x: (yield x), [1, 2])])
tp = lambda: (yield), 1
show("lambda comma tuple", (type(tp).__name__, len(tp)))


# --- 9. ast.parse / ast.dump / ast.unparse parity ---

SOURCES = [
    "lambda: (yield)",
    "lambda x: (yield x) + 10",
    "lambda: (yield from [1, 2, 3])",
    "lambda: (yield 1, 2)",
    "lambda: (yield *a, b)",
    "lambda: (yield not x)",
    "lambda: (yield 1 if a else b)",
    "lambda: (yield lambda: 1)",
    "lambda: (yield (yield))",
    "lambda: (yield from (yield))",
    "lambda: ((yield))",
    "lambda: (x := (yield))",
    "lambda: (yield) if a else b",
    "lambda: (yield) or 1",
    "lambda: (yield)(1)",
    "lambda: (yield)[0]",
    "lambda: (yield).a",
    "lambda: -(yield)",
    "lambda: [(yield), (yield from x)]",
    "lambda: {(yield): (yield)}",
    "lambda: f'{(yield)}'",
    "lambda: f'{yield}'",
    "lambda: lambda: (yield from x)",
    "lambda a=(yield): (yield a)",
    "lambda: (yield), 1",
    "lambda *a, b=1, **c: (yield a, b, c)",
    "lambda: [x for x in (yield)]",
    "lambda: (x for x in (yield))",
    "lambda: [(yield) for x in y]",
    "[lambda: (yield) for x in y]",
    "f(lambda: (yield))",
    "x = lambda: (yield)",
    "def g():\n    f = lambda: (yield)\n    return f\n",
    "def g():\n    return lambda: (yield from x)\n",
    "def g():\n    yield lambda: (yield)\n",
    "class C:\n    m = lambda self: (yield self)\n",
    "(yield)",
    "[(yield)]",
    "f((yield))",
    "x = (yield)",
    "x = y = (yield)",
    "x += (yield)",
    "x: int = (yield)",
    "def g():\n    return (yield)\n",
    "def g():\n    x = (yield 1, 2)\n",
    "def g():\n    x = (yield from y)\n",
    "def g():\n    x = (yield) , 2\n",
    "def g():\n    for x in (yield): pass\n",
    "def g():\n    yield a, b\n",
    "def g():\n    yield (a if b else c)\n",
    "def g():\n    yield from a if b else c\n",
    "def g():\n    yield from lambda: 1\n",
    "def g():\n    yield (yield)\n",
    "def g():\n    yield (x := 1)\n",
    "def g():\n    yield not x\n",
    "def g():\n    x = yield\n",
    "async def g():\n    yield (yield)\n",
]

for src in SOURCES:
    try:
        tree = ast.parse(src)
    except SyntaxError as e:
        print(repr(src), "SyntaxError", e.msg)
        continue
    print(repr(src))
    print("  dump:", ast.dump(tree))
    unparsed = ast.unparse(tree)
    print("  unparse:", repr(unparsed))
    print("  roundtrip:", ast.dump(ast.parse(unparsed)) == ast.dump(tree))

tree = ast.parse("lambda: (yield)")
lam = tree.body[0].value
show("lambda body type", type(lam.body).__name__)
show("lambda body value", lam.body.value)
show("lambda lineno", (lam.lineno, lam.col_offset, lam.end_lineno, lam.end_col_offset))
show("yield positions", (lam.body.lineno, lam.body.col_offset, lam.body.end_lineno, lam.body.end_col_offset))
yf_node = ast.parse("lambda: (yield from xs)").body[0].value.body
show("yield from node", type(yf_node).__name__)
show("yield from positions", (yf_node.col_offset, yf_node.end_col_offset))
show("walk yields", sum(isinstance(n, (ast.Yield, ast.YieldFrom)) for n in ast.walk(ast.parse("lambda: ((yield), (yield from x))"))))

# compile() of the parsed tree gives a generator-flagged lambda code object.
code = compile(ast.parse("lambda: (yield)", mode="eval"), "<ast>", "eval")
lam_fn = eval(code)
show("compiled flag", bool(lam_fn.__code__.co_flags & CO_GENERATOR))
show("compiled value", list(lam_fn()))
show("eval source", list(eval("(lambda: (yield from 'ab'))")()))
show("eval nested", next(eval("lambda x: (yield x * 2)")(21)))


# --- 10. Unparenthesised yield in a lambda is a SyntaxError ---

BAD = [
    "lambda: yield",
    "lambda: yield 1",
    "lambda: yield from x",
    "lambda x: yield x",
    "lambda: yield, 1",
    "f = lambda: yield",
    "lambda a=yield: 1",
    "lambda: lambda: yield",
    "lambda: (yield) = 1",
    "lambda: (yield from)",
    "lambda: (yield, 1)",
    "lambda: (yield for x in y)",
    "lambda: [yield]",
    "lambda: f(yield)",
    "f(yield)",
    "[yield]",
    "(yield, 1)",
    "(yield for x in y)",
    "(yield from)",
]

for src in BAD:
    for runner_name, runner in (("compile", lambda s: compile(s, "<s>", "exec")), ("ast.parse", ast.parse)):
        try:
            runner(src)
        except SyntaxError:
            print(repr(src), runner_name, "SyntaxError")
        else:
            print(repr(src), runner_name, "accepted")

for src in ("lambda: yield 1", "lambda: yield", "lambda: yield from x", "lambda x: yield x"):
    try:
        compile(src, "<s>", "exec")
    except SyntaxError as e:
        show("msg " + src, e.msg)

# `yield` outside a function is only rejected at compile time, not by ast.parse.
try:
    compile("lambda: (yield)", "<s>", "exec")
    show("module-level lambda yield", "compiled")
except SyntaxError as e:
    show("module-level lambda yield", e.msg)

# yield inside comprehensions nested in a lambda is rejected at compile time.
for src in (
    "lambda: [(yield) for x in y]",
    "lambda: ((yield) for x in y)",
    "lambda: {(yield) for x in y}",
):
    try:
        compile(src, "<s>", "exec")
    except SyntaxError as e:
        show(src, e.msg)
    else:
        show(src, "compiled")

# but the first iterable is evaluated in the lambda's own scope
lam_it = lambda: [x for x in (yield)]
gen = lam_it()
next(gen)
try:
    gen.send([1, 2])
except StopIteration as e:
    show("first iterable yield", e.value)

# await inside a lambda body is unaffected by this feature
try:
    compile("lambda: (await x)", "<s>", "exec")
except SyntaxError as e:
    show("await in lambda", e.msg)
else:
    show("await in lambda", "compiled")

print("done")
