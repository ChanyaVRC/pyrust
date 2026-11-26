# Parity test for PEP 634 literal patterns: complex numbers (`1+2j`, `-1-2j`)
# and implicitly concatenated string / bytes literals (`"a" "b"`).  Covers
# runtime matching, ast.parse / ast.dump / ast.unparse, and invalid forms.
import ast

# --- Issue reproducer ---
for v in [1 + 2j, -1 - 2j, -1 + 2j, 1 - 2j, "ab", b"xy"]:
    match v:
        case 1+2j: print("c1")
        case -1-2j: print("c2")
        case -1+2j: print("c3")
        case 1-2j: print("c4")
        case "a" "b": print("str")
        case b"x" b"y": print("bytes")

# --- Complex literal forms, including zero, floats and other radixes ---
def classify(v):
    match v:
        case 0+0j: return "zero"
        case 1.5+2.5j: return "float-parts"
        case -1.5-2.5j: return "neg-float-parts"
        case 0x10+1j: return "hex-real"
        case 1_0+2_0j: return "underscored"
        case 1j: return "pure-imag"
        case -1j: return "neg-imag"
        case 3: return "int"
        case -4.5: return "neg-float"
        case _: return "other"

for v in [0j, 0, 0.0, 1.5 + 2.5j, -1.5 - 2.5j, 16 + 1j, 10 + 20j, 1j, -1j, 3, 3.0, -4.5, 7, "x"]:
    print(repr(v), classify(v))

# --- OR patterns, AS patterns, captures ---
for v in [1 + 2j, -1 - 2j, 5, 1 - 2j]:
    match v:
        case 1+2j | -1-2j as hit:
            print("or-as", hit)
        case (5 | 1-2j) as other:
            print("grouped", other)

match 1 + 2j:
    case 1+2j as z:
        print("as", z)

for v in ["ab", "cd", b"ab", b"cd", "x"]:
    match v:
        case "a" "b" | "c" "d" as s:
            print("str-or", s)
        case b"a" b"b" | b"c" b"d" as bs:
            print("bytes-or", bs)
        case _:
            print("none", v)

# --- Inside sequences, class patterns and with guards ---
match [1 + 2j, "ab", b"xy", -1 - 2j]:
    case [1+2j, "a" "b", b"x" b"y", -1-2j]:
        print("seq-ok")

match (1 + 2j, 3):
    case (1+2j, y):
        print("paren-seq", y)

match 1 + 2j:
    case 1+2j if False:
        print("guard-true")
    case 1+2j if True:
        print("guard-second")

match 1 + 2j:
    case complex(real=1, imag=2):
        print("class-ok")

# --- Single bytes literal and multi-part strings ---
for v in [b"x", "abc", "a b", "line1line2", b"\x00\x01"]:
    match v:
        case b"x": print("bytes1")
        case "a" 'b' """c""": print("three-part")
        case "a b": print("single")
        case "line1" "line2": print("two-lines")
        case b"\x00" b"\x01": print("raw-bytes")

# --- Mapping keys: complex, strings, bytes ---
match {1 + 2j: "c", -1 - 2j: "n", "ab": "s", b"xy": "b"}:
    case {1+2j: a, -1-2j: b, "a" "b": c, b"x" b"y": d}:
        print("mapping", a, b, c, d)

match {1 - 2j: "v"}:
    case {1-2j: w, **rest}:
        print("mapping-rest", w, rest)

# --- Subject is not a number / a string: no match, no error ---
for v in [None, [], {}, 1, 2.0, "1+2j"]:
    match v:
        case 1+2j: print("wrong match")
        case "a" "b": print("wrong match")
        case _: print("no match", repr(v))

# --- ast.parse / ast.dump / ast.unparse ---
def show(pattern):
    src = "match v:\n    case " + pattern + ":\n        pass\n"
    tree = ast.parse(src)
    print(pattern, "->", ast.dump(tree.body[0].cases[0].pattern))
    print("   ", repr(ast.unparse(tree)))

for pat in [
    "1+2j", "-1-2j", "-1+2j", "1-2j", "0+0j", "1.5+2.5j", "0x10+1j", "1_0+2_0j",
    "1j", "-1j", "1+2j | -1-2j", "1+2j as z", "-1+2j as w", "[1+2j, -1-2j]",
    "(1+2j)", "(1+2j, 3)", "{1+2j: x}", "{-1-2j: x, 1-2j: y}",
    '"a" "b"', "b'x' b'y'", "'a' 'b' 'c'", "('a'\n 'b')", "{'a' 'b': x}",
    "{b'a' b'b': x}", "b'x'", "b'x' | b'y'", "r'a' 'b'", "'a' u'b'",
    "'''a''' 'b'", "{1+2j: x, 1-2j: y}",
]:
    show(pat)

# f-strings parse as JoinedStr in ast.parse (they are rejected later, in compile).
for pat in ["f'a'", "'a' f'b'", "f'a' 'b'", "{f'a': x}"]:
    src = "match v:\n    case " + pat + ":\n        pass\n"
    print(repr(pat), ast.dump(ast.parse(src).body[0].cases[0].pattern))

# --- Invalid forms: ast.parse and compile() raise the same SyntaxError ---
invalid = [
    "1+2", "1j+2", "1j+2j", "-1+2", "1+2j+3j", "1+2j-3j", "-1j+1j", "1e3j+1",
    "+1", "+1+2j", "1+-2j", "1+(2j)", "1+j", "1+True", "True+2j", "-None", "-'a'",
    "-b'x'", "x+2j", "a.b+2j", "1+a.b", "1 + 2j + 3j",
    "'a' b'x'", "b'x' 'a'", "b'x' f'a'", "'a' b'x' 'c'",
    "f'a'", "'a' f'b'", "f'a' 'b'", "{f'a': x}",
    "{1+2: x}", "{1j+2: x}", "{-'a': x}", "{'a' b'b': x}", "{x: y}",
    "1 2", "1+2j 3", "'a' 3", "[1+2j+3j]",
    "{1+2j: x, 1+2j: y}", "{'a' 'b': x, 'ab': y}", "{b'ab': x, b'a' b'b': y}",
    "{1: x, 1+0j: y}", "{-1: x, -1+0j: y}", "{1+2j: x, 1-2j: y, 1+2j: z}",
]
for pat in invalid:
    src = "match v:\n    case " + pat + ":\n        pass\n"
    try:
        ast.parse(src)
        parsed = "parsed"
    except SyntaxError as e:
        parsed = "ast: " + e.msg
    try:
        compile(src, "<t>", "exec")
        compiled = "compiled"
    except SyntaxError as e:
        compiled = "compile: " + e.msg
    print(repr(pat), "|", parsed, "|", compiled)

# A case header that ends at the newline without a colon.
for src in ["match v:\n    case 1+2j\n        pass\n", "match v:\n    case 1+2j if x\n        pass\n"]:
    try:
        compile(src, "<t>", "exec")
    except SyntaxError as e:
        print("no colon:", e.msg)

# --- exec() of match using the new literals ---
ns = {}
exec(
    "def f(v):\n"
    "    match v:\n"
    "        case -1+0j: return 'neg-one'\n"
    "        case 2-3j: return 'two-minus-three'\n"
    "        case b'a' b'b': return 'bytes'\n"
    "        case 'x' 'y': return 'str'\n"
    "    return 'none'\n",
    ns,
)
for v in [-1, -1 + 0j, 2 - 3j, b"ab", "xy", "z"]:
    print(repr(v), ns["f"](v))
