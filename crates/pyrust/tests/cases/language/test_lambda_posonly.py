# Positional-only parameter marker `/` in lambda parameter lists (#3349).
import ast
import inspect

# --- Issue reproducer ---
f = lambda x, /, y=2, *, z=3: (x, y, z)
print(f(1, y=20, z=30))
print(f(1))
print(f(1, 5))
try:
    f(x=1)
except TypeError as e:
    print("posonly:", type(e).__name__, e)
try:
    f()
except TypeError as e:
    print(e)
try:
    f(1, 2, 3)
except TypeError as e:
    print(e)

# --- Runtime shapes ---
print((lambda a, /: a)(7))
print((lambda a, /,: a)(7))
print((lambda a, b, /: (a, b))(1, 2))
print((lambda a, /, b: (a, b))(1, b=2))
print((lambda a, b=2, /, c=3: (a, b, c))(1))
print((lambda a, b=2, /, c=3: (a, b, c))(1, c=9))
print((lambda a, /, *args: (a, args))(1, 2, 3))
print((lambda a, /, **kw: (a, kw))(1, a=2, b=3))
print((lambda a, /, b, *c, d=1, **e: (a, b, c, d, e))(1, 2, 3, d=4, x=5))
print((lambda a, b=2, /, c=3, *d, e, **k: (a, b, c, d, e, k))(1, e=5, q=9))
print((lambda a, /, *, k: (a, k))(1, k=2))
print((lambda a, /, *, k=0: (a, k))(1))
print((lambda a=1, /, b=2: (a, b))())
print((lambda a=1, /, b=2: (a, b))(b=9))

# --- Introspection ---
code = f.__code__
print(code.co_posonlyargcount, code.co_argcount, code.co_kwonlyargcount)
print(code.co_varnames)
print(inspect.signature(f))
print(inspect.signature(lambda a, b=2, /, c=3, *d, e, **k: 0))
print(f.__defaults__, f.__kwdefaults__)
for p in inspect.signature(f).parameters.values():
    print(p.name, p.kind)

# --- Nesting / defaults evaluated in order ---
g = lambda a, /: (lambda b, /, c=a: (a, b, c))
print(g(1)(2))
print(g(1)(2, 3))
print(sorted(["b", "A", "c"], key=lambda s, /: s.lower()))
print(list(map(lambda a, b, /: a + b, [1, 2], [3, 4])))

# --- Same behavior as def ---
def d(x, /, y=2, *, z=3):
    return (x, y, z)


print(d(1, y=20, z=30) == f(1, y=20, z=30))
print(inspect.signature(d))

# --- ast.parse / ast.dump / ast.unparse ---
SOURCES = [
    "lambda x, /, y=1: x + y",
    "lambda x, /, y=2, *, z=3: (x, y, z)",
    "lambda a, /: a",
    "lambda a, b, /: a",
    "lambda a, /, b: a",
    "lambda a=1, /, b=2: a",
    "lambda a, /, *args: a",
    "lambda a, /, **kw: a",
    "lambda a, /, *, b: a",
    "lambda a, /, b, *c, d=1, **e: a",
    "lambda a, b=1, /, c=2, *d, e, f=3, **g: a",
    "lambda a, /,: a",
    "lambda: 0",
    "lambda a, b: a",
]
for src in SOURCES:
    tree = ast.parse(src)
    lam = tree.body[0].value
    print(src, "->", ast.unparse(tree))
    print(ast.dump(lam.args))
    print(ast.dump(tree))
    print(ast.unparse(ast.parse(ast.unparse(tree))) == ast.unparse(tree))

tree = ast.parse("lambda x, /, y=1: x + y")
args = tree.body[0].value.args
print([a.arg for a in args.posonlyargs], [a.arg for a in args.args])
print([ast.dump(x) for x in args.defaults])
print(ast.dump(tree, indent=2))
print(ast.unparse(tree))

# compile() of an AST containing posonlyargs in a Lambda.
code = compile(ast.parse("lambda x, /, y=1: x + y", mode="eval"), "<ast>", "eval")
h = eval(code)
print(h(1), h(1, 5), h(1, y=7))
try:
    h(x=1)
except TypeError as e:
    print(e)

# --- Invalid forms (SyntaxError messages) ---
for src in [
    "lambda /: 1",
    "lambda a, /, b, /: 1",
    "lambda a, /, /: 1",
    "lambda *a, /: 1",
    "lambda *, /: 1",
    "lambda a, *, /: 1",
    "lambda *, a, /: 1",
    "lambda a, /, *a2, /: 1",
    "lambda a=1, /, b: 1",
    "lambda a, b=1, /, c: 1",
    "lambda **k, /: 1",
    "lambda a, /, *: 1",
    "def f(/): pass",
    "def f(a, /, b, /): pass",
    "def f(*a, /): pass",
    "def f(a=1, /, b): pass",
]:
    mode = "eval" if src.startswith("lambda") else "exec"
    try:
        compile(src, "<s>", mode)
        print("OK", src)
    except SyntaxError as e:
        print(src, "=>", e.msg)
