import ast

# 1. Class hierarchy, _fields, _attributes, __match_args__, constructors
print("is_sub_stmt:", issubclass(ast.FunctionDef, ast.stmt), issubclass(ast.stmt, ast.AST))
print("is_sub_expr:", issubclass(ast.BinOp, ast.expr), issubclass(ast.expr, ast.AST))
print("is_sub_op:", issubclass(ast.Add, ast.operator), issubclass(ast.And, ast.boolop))
print("is_sub_pat:", issubclass(ast.MatchClass, ast.pattern), issubclass(ast.TypeVar, ast.type_param))
print("match_args:", ast.BinOp.__match_args__)

n = ast.Name("foo", ast.Load(), lineno=1, col_offset=0)
print("ctor:", n.id, type(n.ctx).__name__, n.lineno, n.col_offset)
try:
    ast.Name("a", ast.Load(), "extra")
    print("too_many_args: FAIL")
except TypeError:
    print("too_many_args: TypeError")

# Structural pattern matching on AST nodes
match ast.parse("1 + 2", mode="eval").body:
    case ast.BinOp(ast.Constant(a), ast.Add(), ast.Constant(b)):
        print("ast_match:", a, b)

# 2. Comprehensive ast.dump(ast.parse(...)) parity across Python 3.12 grammar
SAMPLES = [
    # Assignments & annotations
    "x = 1",
    "a = b = c = 0",
    "a, *b, c = [1, 2, 3, 4]",
    "[a, b] = (1, 2)",
    "x: int = 1",
    "x: int",
    "(x): int = 1",
    "obj.attr: str = 'hi'",
    "arr[0]: int",
    "x += 1; y **= 2; z //= 3; w @= m",
    # Functions & async functions & PEP 695
    "@dec1\n@dec2(1)\ndef foo[T, U: int, V: (int, str), *Ts, **P](a, b=1, /, c: int = 2, *args: str, d, e=3, **kwargs) -> bool:\n    '''Docstring.'''\n    return True",
    "async def bar(x):\n    await x\n    async for i in x:\n        pass\n    async with x as y:\n        pass",
    # Classes & PEP 695
    "@cls_dec\nclass Box[T](Base, *extra, metaclass=Meta, flag=True, **kw):\n    x = 1",
    # TypeAlias (PEP 695)
    "type Point = tuple[float, float]",
    "type Matrix[T: float] = list[list[T]]",
    # Control flow
    "if a:\n    1\nelif b:\n    2\nelse:\n    3",
    "while x:\n    break\nelse:\n    continue",
    "for k, v in items:\n    pass\nelse:\n    pass",
    "with a() as x, b() as y:\n    pass",
    "with (a() as x, b() as y):\n    pass",
    # Try / except / else / finally & TryStar (PEP 654)
    "try:\n    risky()\nexcept ValueError as e:\n    handle(e)\nexcept (TypeError, KeyError):\n    pass\nexcept:\n    raise\nelse:\n    ok()\nfinally:\n    cleanup()",
    "try:\n    task()\nexcept* ValueError as eg:\n    handle(eg)\nexcept* KeyError:\n    pass",
    # Match / case (PEP 634)
    """match cmd:
    case None | True | False:
        pass
    case -1 | 1 + 2j | "quit":
        pass
    case [first, *rest, last] if first > 0:
        pass
    case {"type": "move", "pos": (x, y), **extra}:
        pass
    case Point(0, y=y_val) as pt:
        pass
    case mod.CONST:
        pass
    case _:
        pass""",
    # Imports, global, nonlocal, del, assert, raise
    "import os, sys as system",
    "from pkg.sub import a, b as c",
    "from . import x",
    "from ..pkg import *",
    "global g1, g2; nonlocal n1",
    "del a, b.c, d[0]",
    "assert x > 0, 'must be positive'",
    "raise RuntimeError('boom') from err",
    # Expressions: boolops, binops, unaryops, comparisons, ternary, lambda, walrus
    "a and b and c",
    "(a and b) and c",
    "a or b or c",
    "not a and -b + ~c * +d",
    "-2 ** 3",
    "2 ** -3",
    "1 < x <= 10 == y != z is not None in s not in t",
    "a if cond else b if cond2 else c",
    "lambda x, y=1, /, z=2, *a, k=3, **kw: x + y",
    "(n := len(xs)) > 0",
    # Collections & comprehensions
    "[1, *a, 2]",
    "(1,)",
    "()",
    "{1, 2, 3}",
    "{'a': 1, **d, 'b': 2}",
    "[x * 2 for x in xs if x > 0 if x < 10]",
    "{k: v for k, v in pairs}",
    "{x for row in grid for x in row}",
    "sum(x for x in xs)",
    "[x async for x in aiter]",
    # Subscripts & slices
    "a[1]",
    "a[1:2]",
    "a[1:2:3]",
    "a[:]",
    "a[::2]",
    "a[1:2, 3:4]",
    # Yield / yield from
    "def gen():\n    yield\n    yield 1\n    yield 1, 2\n    yield from other()",
    # F-strings & string constants
    'f""',
    'f"plain"',
    'f"hello {name!r:>10}"',
    'f"{val:{width}.{prec}f}"',
    'f"{x=}"',
    'f"{x = !s}"',
    'f"{x=:10}"',
    '"a" f"{x}" "b"',
    'u"unicode"',
    'b"bytes" b"2"',
    '...',
]

for idx, src in enumerate(SAMPLES):
    tree = ast.parse(src)
    print(f"CASE {idx}: {ast.dump(tree)}")

# 3. Modes: eval, single, func_type, bytes source, AST passthrough
print("eval:", ast.dump(ast.parse("1 + 2", mode="eval")))
print("single:", ast.dump(ast.parse("x = 1", mode="single")))
print("func_type:", ast.dump(ast.parse("(int, str) -> bool", mode="func_type")))
print("bytes:", ast.dump(ast.parse(b"# coding: utf-8\nx = 'caf\xc3\xa9'\n")))
existing = ast.parse("x = 1")
print("passthrough:", ast.parse(existing) is existing)

# 4. type_comments=True
tc_src = """x = 1  # type: int
y = 2  # type: ignore[assignment]
def f(a):  # type: (int) -> int
    return a
"""
print("type_comments:", ast.dump(ast.parse(tc_src, type_comments=True)))

# 5. Location attributes (including UTF-8 byte offsets)
loc_src = "α = 1 + 2\n"
print("attrs:", ast.dump(ast.parse(loc_src), include_attributes=True))

# 6. dump options: indent & annotate_fields
t_small = ast.parse("x = 1 + 2")
print("dump_no_ann:", ast.dump(t_small, annotate_fields=False))
print("dump_indent:\n" + ast.dump(t_small, indent=2))

# 7. SyntaxError on invalid input
for bad in ["def (", "1 = 2", "x = (1 + 2", "try:\n    pass\nexcept ValueError:\n    pass\nexcept* TypeError:\n    pass"]:
    try:
        ast.parse(bad, filename="bad.py")
        print("syntax_err: FAIL", repr(bad))
    except SyntaxError as e:
        print("syntax_err:", type(e).__name__, e.filename, e.lineno is not None)

# 8. literal_eval
for lit in [
    "42",
    "-7",
    "+3.5",
    "1 + 2j",
    "-1 - 2j",
    "1.5 - 0.5j",
    "'hello'",
    "b'abc'",
    "True",
    "False",
    "None",
    "...",
    "(1, 2, 3)",
    "[1, [2, 3]]",
    "{'a': 1, 'b': [True, None]}",
    "{1, 2, 3}",
    "set()",
    "   123",
]:
    print("lit:", lit, "->", repr(ast.literal_eval(lit)))

for bad_lit in ["x", "1 + 2", "f()", "True + 1", "-True", "{**{}}", "__import__('os')"]:
    try:
        ast.literal_eval(bad_lit)
        print("bad_lit: FAIL", bad_lit)
    except ValueError:
        print("bad_lit: ValueError", bad_lit)

# 9. walk, iter_fields, iter_child_nodes
wtree = ast.parse("a = b + 1")
print("walk_types:", [type(x).__name__ for x in ast.walk(wtree)])
print("iter_fields:", [(k, type(v).__name__) for k, v in ast.iter_fields(wtree.body[0])])
print("iter_children:", [type(x).__name__ for x in ast.iter_child_nodes(wtree.body[0])])

# 10. NodeVisitor and NodeTransformer
class NameCollector(ast.NodeVisitor):
    def __init__(self):
        self.names = []

    def visit_Name(self, node):
        self.names.append(node.id)
        self.generic_visit(node)


nc = NameCollector()
nc.visit(ast.parse("x = a + b(c)"))
print("visitor_names:", nc.names)


class DoubleInts(ast.NodeTransformer):
    def visit_Constant(self, node):
        if isinstance(node.value, int) and not isinstance(node.value, bool):
            return ast.copy_location(ast.Constant(value=node.value * 2), node)
        return node


tr = DoubleInts().visit(ast.parse("x = 3 + 4"))
ast.fix_missing_locations(tr)
print("transformer:", ast.dump(tr))

# 11. fix_missing_locations, increment_lineno, copy_location
synth = ast.Module(body=[ast.Expr(value=ast.Constant(value=99))], type_ignores=[])
ast.fix_missing_locations(synth)
print("fixed_loc:", synth.body[0].lineno, synth.body[0].col_offset, synth.body[0].value.lineno)
ast.increment_lineno(synth, 10)
print("inc_loc:", synth.body[0].lineno, synth.body[0].value.lineno)

# 12. get_docstring
doc_tree = ast.parse('''"""Module doc.

    Indented line.
"""
def f():
    """Func doc.

    Detail.
    """
    pass

class C:
    """Class doc."""
    pass

def no_doc():
    return 1
''')
print("mod_doc:", repr(ast.get_docstring(doc_tree)))
print("mod_doc_raw:", repr(ast.get_docstring(doc_tree, clean=False)))
print("fn_doc:", repr(ast.get_docstring(doc_tree.body[1])))
print("cls_doc:", repr(ast.get_docstring(doc_tree.body[2])))
print("no_doc:", ast.get_docstring(doc_tree.body[3]))
try:
    ast.get_docstring(ast.parse("x = 1").body[0])
    print("bad_doc: FAIL")
except TypeError:
    print("bad_doc: TypeError")

# 13. get_source_segment
seg_src = "def f(x):\n    return x + 1\n"
seg_tree = ast.parse(seg_src)
ret_stmt = seg_tree.body[0].body[0]
print("seg_single:", repr(ast.get_source_segment(seg_src, ret_stmt)))
print("seg_multi:", repr(ast.get_source_segment(seg_src, seg_tree.body[0])))
multi_indented = "if True:\n    def g():\n        return 1\n"
g_node = ast.parse(multi_indented).body[0].body[0]
print("seg_padded:", repr(ast.get_source_segment(multi_indented, g_node, padded=True)))

# 14. ast.unparse roundtrips (parse -> unparse -> parse matches original AST dump)
ROUNDTRIP_SAMPLES = [
    "x = 1 + 2 * 3",
    "y = (1 + 2) * 3",
    "z = 2 ** 3 ** 4",
    "w = (2 ** 3) ** 4",
    "a = -2 ** 3",
    "b = (-2) ** 3",
    "c = a and b or c",
    "d = a and (b or c)",
    "e = 1 .real",
    "def f[T: int, *Ts, **P](a, /, b=1, *args, c=2, **kw) -> list[T]:\n    return [a, b, c]",
    "class C(Base, metaclass=Meta):\n    pass",
    "type Alias[T] = dict[str, T]",
    "if a:\n    x = 1\nelif b:\n    x = 2\nelse:\n    x = 3",
    "try:\n    f()\nexcept* ValueError as e:\n    g(e)\nfinally:\n    h()",
    "match v:\n    case [1, *rest] if rest:\n        pass\n    case Point(x=1, y=y) as p:\n        pass\n    case _:\n        pass",
    "[x * y for x in xs for y in ys if x != y]",
    "{k: v for k, v in items}",
    "f'hello {name!r:>10}'",
    "with a() as x, b() as y:\n    pass",
    "from ..pkg import a as b",
]

for idx, src in enumerate(ROUNDTRIP_SAMPLES):
    t1 = ast.parse(src)
    unparsed = ast.unparse(t1)
    t2 = ast.parse(unparsed)
    ok = ast.dump(t1) == ast.dump(t2)
    print(f"ROUNDTRIP {idx}: {ok}")
    if not ok:
        print("  ORIG:", ast.dump(t1))
        print("  UNPARSED:", repr(unparsed))
        print("  REPARSED:", ast.dump(t2))
