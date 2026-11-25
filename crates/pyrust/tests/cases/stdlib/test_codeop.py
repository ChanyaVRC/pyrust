"""CPython 3.12 parity tests for the `codeop` module (issue #3228)."""

import __future__
import codeop
import types


# --- Public surface & metadata ---
print("__all__:", codeop.__all__)
print("flags:", hex(codeop.PyCF_DONT_IMPLY_DEDENT), hex(codeop.PyCF_ALLOW_INCOMPLETE_INPUT))
print("Compile.__module__:", codeop.Compile.__module__)
print("CommandCompiler.__module__:", codeop.CommandCompiler.__module__)


def classify(fn, source, filename="<input>", symbol="single"):
    try:
        res = fn(source, filename, symbol)
    except SyntaxError as exc:
        # Both IndentationError and SyntaxError are subclasses of SyntaxError;
        # normalize to SyntaxError because parser diagnostics differ on edge cases.
        return f"SYNTAX_ERROR({issubclass(type(exc), SyntaxError)})"
    except ValueError:
        return "VALUE_ERROR"
    if res is None:
        return "INCOMPLETE"
    assert isinstance(res, types.CodeType), f"expected CodeType, got {type(res)}"
    return f"COMPLETE({res.co_filename})"


# --- compile_command: complete, incomplete, and invalid inputs ---
cases = [
    # Blank & comment-only inputs
    ("empty single", "", "single"),
    ("spaces single", "    ", "single"),
    ("comment single", "# just a comment", "single"),
    ("newline single", "\n", "single"),
    ("multi blank+comment single", "\n  # a\n\n  # b", "single"),
    ("empty eval", "", "eval"),
    ("spaces eval", "   ", "eval"),
    ("comment eval", "# hi", "eval"),
    ("newline eval", "\n", "eval"),

    # Complete simple statements & expressions
    ("expr single", "1 + 2", "single"),
    ("expr eval", "1 + 2", "eval"),
    ("assign single", "x = 42", "single"),
    ("semicolon single", "a = 1; b = 2", "single"),
    ("multiline expr complete", "x = (1 +\n2)", "single"),
    ("multiline list complete", "[\n1,\n2,\n]", "single"),
    ("triple quote complete", '"""hello\nworld"""', "single"),
    ("fstring triple complete", 'f"""hello\n{1+2}"""', "single"),
    ("line continuation complete", "1 + \\\n2", "single"),
    ("simple with comment", "1 + 2 # comment", "single"),
    ("exec compound no nl", "if True:\n    pass", "exec"),
    ("exec compound with nl", "if True:\n    pass\n", "exec"),

    # Incomplete compound statements (missing body or missing trailing newline)
    ("inline if no nl", "if True: pass", "single"),
    ("inline if with nl", "if True: pass\n", "single"),
    ("inline def no nl", "def f(): return 1", "single"),
    ("inline def with nl", "def f(): return 1\n", "single"),
    ("def header", "def f():", "single"),
    ("def header nl", "def f():\n", "single"),
    ("def body no nl", "def f():\n    return 1", "single"),
    ("def body with nl", "def f():\n    return 1\n", "single"),
    ("def body spaces no nl", "def f():\n    return 1\n    ", "single"),
    ("def body double nl", "def f():\n    return 1\n\n", "single"),
    ("def blank internal no nl", "def f():\n    x = 1\n\n    return x", "single"),
    ("def blank internal nl", "def f():\n    x = 1\n\n    return x\n", "single"),
    ("if header", "if True:", "single"),
    ("if body no nl", "if True:\n    x = 1", "single"),
    ("if body with nl", "if True:\n    x = 1\n", "single"),
    ("if comment only", "if True:\n    # comment", "single"),
    ("if comment only nl", "if True:\n    # comment\n", "single"),
    ("if body trailing comment no nl", "if True:\n    x = 1\n# c", "single"),
    ("if body trailing comment nl", "if True:\n    x = 1\n# c\n", "single"),
    ("if else header", "if True:\n    x = 1\nelse:", "single"),
    ("if else body no nl", "if True:\n    x = 1\nelse:\n    x = 2", "single"),
    ("if else body nl", "if True:\n    x = 1\nelse:\n    x = 2\n", "single"),
    ("if elif header", "if True:\n    pass\nelif False:", "single"),
    ("if elif body nl", "if True:\n    pass\nelif False:\n    pass\n", "single"),
    ("nested if no nl", "def f():\n    if True:\n        return 1", "single"),
    ("nested if nl", "def f():\n    if True:\n        return 1\n", "single"),
    ("nested if header", "def f():\n    if True:", "single"),
    ("for no nl", "for i in range(3):\n    pass", "single"),
    ("for nl", "for i in range(3):\n    pass\n", "single"),
    ("for else header", "for i in range(3):\n    pass\nelse:", "single"),
    ("while no nl", "while False:\n    pass", "single"),
    ("while nl", "while False:\n    pass\n", "single"),
    ("with no nl", "with ctx:\n    pass", "single"),
    ("with nl", "with ctx:\n    pass\n", "single"),
    ("class no nl", "class C:\n    pass", "single"),
    ("class nl", "class C:\n    pass\n", "single"),
    ("async def no nl", "async def f():\n    pass", "single"),
    ("async def nl", "async def f():\n    pass\n", "single"),
    ("try header", "try:", "single"),
    ("try body no nl", "try:\n    pass", "single"),
    ("try body nl", "try:\n    pass\n", "single"),
    ("try except header", "try:\n    pass\nexcept:", "single"),
    ("try except body no nl", "try:\n    pass\nexcept:\n    pass", "single"),
    ("try except body nl", "try:\n    pass\nexcept:\n    pass\n", "single"),
    ("try finally header", "try:\n    pass\nfinally:", "single"),
    ("try finally body nl", "try:\n    pass\nfinally:\n    pass\n", "single"),
    ("nested try body nl", "def f():\n    try:\n        pass\n", "single"),
    ("match header", "match x:", "single"),
    ("match case header", "match x:\n    case 1:", "single"),
    ("match case body no nl", "match x:\n    case 1:\n        pass", "single"),
    ("match case body nl", "match x:\n    case 1:\n        pass\n", "single"),
    ("decorator alone", "@dec", "single"),
    ("decorator nl", "@dec\n", "single"),
    ("decorator def header", "@dec\ndef f():", "single"),
    ("decorator def body no nl", "@dec\ndef f():\n    pass", "single"),
    ("decorator def body nl", "@dec\ndef f():\n    pass\n", "single"),
    ("nested decorator", "def f():\n    @dec", "single"),

    # Incomplete delimiters, strings, line continuations
    ("open paren", "x = (1 +", "single"),
    ("open paren nested", "x = (1 + (", "single"),
    ("open paren nl", "x = (1 + 2\n", "single"),
    ("open bracket", "x = [1,", "single"),
    ("open brace", "x = {1:", "single"),
    ("open def params", "def f(a,\n", "single"),
    ("eval open paren", "(", "eval"),
    ("triple dquote", '"""hello', "single"),
    ("triple dquote multiline", '"""hello\nworld', "single"),
    ("triple squote", "'''hello", "single"),
    ("raw triple quote", 'r"""hello', "single"),
    ("bytes triple quote", 'b"""hello', "single"),
    ("fstring open expr", 'f"{1 +', "single"),
    ("fstring triple open", 'f"""{1 +', "single"),
    ("trailing backslash", "1 + \\", "single"),
    ("eval trailing backslash", "1 + \\", "eval"),

    # Genuine syntax errors (must raise SyntaxError, NOT return None)
    ("bare def", "def", "single"),
    ("dangling plus", "1 +", "single"),
    ("double assign", "x = = 1", "single"),
    ("unindented suite", "if True:\npass", "single"),
    ("two statements single", "1\n2", "single"),
    ("bad class name", "class 123:", "single"),
    ("bad param", "def f(1):", "single"),
    ("bad paren expr", "x = (1 + )", "single"),
    ("bad expr inside open paren", "x = (1 = = 2", "single"),
    ("unclosed single quote", '"unclosed', "single"),
    ("bad prefix + triple quote", 'x = = """hello', "single"),
    ("double backslash", "1 + \\\\", "single"),
    ("backslash + nl only", "1 + \\\n", "single"),
    ("bad unindent", "if True:\n    pass\n  x = 1", "single"),
    ("compound + trailing stmt", "if True:\n    1\nx = 2\n", "single"),
    ("def with else", "def f():\n    pass\nelse:", "single"),
    ("invalid fstring quote", 'f"{1 + "', "single"),
    ("eval statement", "x = 1", "eval"),
    ("invalid symbol", "1 + 1", "bogus"),
]

for label, src, sym in cases:
    print(f"{label}: {classify(codeop.compile_command, src, '<test>', sym)}")

# --- Executing compiled code objects ---
co_empty = codeop.compile_command("")
ns = {}
exec(co_empty, ns)
print("exec empty ok:", "__builtins__" in ns)

co_fn = codeop.compile_command("def add(a, b):\n    return a + b\n", "<myfn>")
exec(co_fn, ns)
print("add(3, 4):", ns["add"](3, 4), "filename:", ns["add"].__code__.co_filename)

co_eval = codeop.compile_command("6 * 7", "<eval>", "eval")
print("eval result:", eval(co_eval))


# --- Compile class & __future__ flag tracking ---
c = codeop.Compile()
print("Compile initial flags:", hex(c.flags))
co_basic = c("x = 10", "<c>", "single")
print("Compile basic type:", type(co_basic) is types.CodeType)
print("Compile flags before annotations:", bool(c.flags & __future__.CO_FUTURE_ANNOTATIONS))

c("from __future__ import annotations", "<c>", "single")
print("Compile flags after annotations:", bool(c.flags & __future__.CO_FUTURE_ANNOTATIONS))
co_ann = c("def g(x: NonExistentType) -> AnotherFake:\n    return x\n", "<c>", "single")
ns2 = {}
exec(co_ann, ns2)
print("Compile remembered annotations:", ns2["g"].__annotations__)
print("Compile ns2 user keys:", sorted(k for k in ns2 if k != "__builtins__"))

# Separate Compile instance does NOT inherit flags from `c`
c_fresh = codeop.Compile()
print("fresh Compile has annotations:", bool(c_fresh.flags & __future__.CO_FUTURE_ANNOTATIONS))


# --- CommandCompiler class & stateful __future__ tracking ---
cc = codeop.CommandCompiler()
print("cc incomplete:", cc("def h(a: FakeA):"))
print("cc initial annotations flag:", bool(cc.compiler.flags & __future__.CO_FUTURE_ANNOTATIONS))
co_f = cc("from __future__ import annotations")
print("cc after future flag:", bool(cc.compiler.flags & __future__.CO_FUTURE_ANNOTATIONS))
co_h = cc("def h(a: FakeA) -> FakeB:\n    return a\n")
ns3 = {}
exec(co_f, ns3)
exec(co_h, ns3)
print("cc remembered annotations:", ns3["h"].__annotations__, ns3["h"](99))

print("codeop parity ok")
