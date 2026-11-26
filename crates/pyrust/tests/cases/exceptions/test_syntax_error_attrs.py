# Compile-time SyntaxError / IndentationError / TabError instances carry
# CPython 3.12's location: `args == (msg, (filename, lineno, offset, text,
# end_lineno, end_offset))` and the matching attributes (issue #3402).
#
# `full` compares the whole exception; `where` compares only the location for
# errors whose message wording or subclass still differs from CPython (outside
# #3402).  Filenames are pseudo names (`<test>`) so CPython never reads `text`
# back from a file on disk, except in the import section, which does on purpose.

import importlib
import os
import shutil
import sys
import tempfile
import traceback


def attrs(exc):
    return (exc.filename, exc.lineno, exc.offset, exc.text, exc.end_lineno, exc.end_offset)


def full(label, src, mode="exec", filename="<test>"):
    try:
        compile(src, filename, mode)
    except SyntaxError as exc:
        print(label, type(exc).__name__)
        print("   args:", ascii(exc.args))
        print("  attrs:", ascii(attrs(exc)))
    else:
        print(label, "no error")


def where(label, src, mode="exec"):
    try:
        compile(src, "<test>", mode)
    except SyntaxError as exc:
        print(label, len(exc.args), ascii(attrs(exc)))
    else:
        print(label, "no error")


def run(label, fn, wording=True):
    try:
        fn()
    except SyntaxError as exc:
        msg = ascii(exc.args[0]) if wording else "-"
        print(label, type(exc).__name__, msg, ascii(attrs(exc)))
    else:
        print(label, "no error")


print("--- issue reproducer")
for src, mode in [
    ("x = (1,\n2 +", "exec"),
    ("1 +", "eval"),
    ("def f(:\n    pass", "exec"),
    ("a = 1\nb = [1, 2\n", "exec"),
]:
    try:
        compile(src, "s.py", mode)
    except SyntaxError as e:
        print(e.filename, e.lineno, e.offset, repr(e.text))

print("--- unclosed brackets")
full("paren-multiline", "x = (1,\n2 +")
full("call-exec", "foo(")
full("call-eval", "foo(", "eval")
full("call-eval-nl", "foo(\n", "eval")
full("list-line2", "a = 1\nb = [1, 2\n")
full("dict", "x = {1: 2,\n  3: 4\n")
full("nested", "x = (1,\n  [2,\n  3]\n")
full("innermost", "f(a, [b,\n  c")
full("open-last-line", "x = (\n")
full("open-blank-after", "x = (\n\n")
full("brace-blanks", "d = {\n\n\n")
full("bare-eval", "(", "eval")
full("single", "(1,", "single")
full("crlf", "x = (1,\r\n2\r\n")
full("unicode-col", "\u00e9 = (\u00e9,\n")

print("--- number literals (tokenizer errors)")
for src in [
    "x = 0777",
    "x = 00777",
    "x = 0_7",
    "x = 1_",
    "x = 1__0",
    "x = 1.5_",
    "x = 1e5_",
    "x = 1_000_",
    "x = 0x",
    "x = 0x_",
    "x = 0x1_",
    "x = 0o",
    "x = 0b",
    "y = 1\nx = 2 + 1_",
]:
    full(ascii(src), src)

print("--- indentation")
full("unindent", "if 1:\n    x = 1\n  y = 2")
full("unindent-nl", "if 1:\n    x = 1\n  y = 2\n")
full("unindent-crlf", "if 1:\r\n    x = 1\r\n  y = 2\r\n")
full("tab-after-spaces", "if 1:\n\tpass\n        pass\n")
full("tab-deeper", "if 1:\n        a\n\tb\n")
deep = "".join(" " * i + "if 1:\n" for i in range(101)) + " " * 101 + "pass\n"
try:
    compile(deep, "<test>", "exec")
except SyntaxError as exc:
    print("too-deep", type(exc).__name__, ascii(exc.args[0]), exc.lineno, exc.offset,
          exc.end_lineno, exc.end_offset, exc.text == " " * 100 + "if 1:\n")

print("--- invalid characters")
full("dollar", "x = $")
full("question-line2", "a = 1\nb = ?\n")
full("dollar-mid", "x = 1\ny = 2 $ 3\n")
full("backtick", "x = `a`")

print("--- targets reported on the target")
full("assign-literal", "1 = 2")
full("assign-literal-nl", "1 = 2\n")
full("del-literal", "del 1")
full("del-call", "del f()")
full("del-second", "del a, 1")

print("--- literals and f-strings")
full("unknown-name-escape", "x = '\\N{nope}'")
full("fstring-conversion", "x = f'{a!z}'")
full("fstring-missing-conversion", "x = f'{a!}'")
full("fstring-empty-field", "x = f'{}'")

print("--- parameters")
full("default-order", "def f(a=1, b): pass")
full("default-order-annotated", "def f(a=1, b: int):\n    pass")
full("lambda-default-order", "lambda a=1, b: 0", "eval")

print("--- location only")
where("eval-implicit-newline", "1 +", "eval")
where("eval-real-newline", "1 +\n", "eval")
where("eval-trailing-space", "1 +  ", "eval")
where("eval-leading-newline", "\n1 +", "eval")
where("single-implicit-newline", "1 +", "single")
where("single-real-newline", "1 +\n", "single")
where("exec-newline-appended", "x = 1 +")
where("exec-newline", "x = 1 +\n")
where("bad-param", "def f(:\n    pass")
where("line2", "x = 1\ny = 2 +\nz = 3\n")
where("import-as", "import a.b as")
where("annotated-chain", "x: int = 1 = 2")
where("star-call", "print(*)")
where("block-at-eof", "for x in y:\n")
where("continuation-at-eof", "x = \\")
where("unexpected-indent", "pass\n  pass\n")
where("missing-block", "if x:\npass\n")
where("unicode", "\u00e9 = 1\nx = \u00e9 +\n")
where("crlf", "x = 1\r\ny = 2 +\r\n")
where("assign-call", "f() = 1")
where("assign-binop", "x + 1 = 2")
where("assign-none", "None = 1")
where("assign-chained", "x = 1 = 2")
where("aug-literal", "1 += 2")
where("unterminated", "x = 'abc")
where("unterminated-line2", "y = 1\nx = 'ab\nz = 3")
where("unterminated-triple", "x = '''abc\ndef")
where("unterminated-triple-line2", "x = 1\ny = '''a\nb\nc")
where("unterminated-bytes", "x = b'abc")
where("unterminated-fstring", "x = f'abc")
where("bad-escape", "x = '\\x1'")
where("bad-escape-line2", "y = 1\nx = 'ab\\u12'")
where("bytes-escape", "x = b'\\x1'")
where("bytes-non-ascii", "x = b'\u00e9'")

print("--- exec / eval builtins")
# eval() strips leading spaces and tabs before compiling, like CPython.
run("eval", lambda: eval("1 +"), wording=False)
run("eval-leading-blanks", lambda: eval("  1 +"), wording=False)
run("eval-leading-newline", lambda: eval("\n1 +"), wording=False)
run("eval-trailing-newlines", lambda: eval("1 +\n\n"), wording=False)
run("eval-unclosed", lambda: eval("(1,"))
run("exec-unclosed", lambda: exec("x = 1\ny = (2 +\n"))
run("exec", lambda: exec("1 +"), wording=False)
run("exec-unindent", lambda: exec("if 1:\n  a\n b\n"))
run("exec-number", lambda: exec("x = 0x"))
run("exec-code-filename", lambda: exec(compile("x = (", "<named>", "exec")))

print("--- exception object")
try:
    compile("x = (1,\n2 +", "<test>", "exec")
except SyntaxError as exc:
    print(str(exc))
    print(exc.msg, exc.print_file_and_line)
    print(isinstance(exc.args[1], tuple), exc.args[1] == attrs(exc))
try:
    compile("if 1:\n\tpass\n        pass\n", "<test>", "exec")
except SyntaxError as exc:
    print(str(exc), isinstance(exc, IndentationError), isinstance(exc, TabError))
try:
    compile("if 1:\n    x = 1\n  y = 2\n", "<test>", "exec")
except IndentationError as exc:
    print(str(exc), type(exc).__name__)
nested = os.path.join("pkg", "sub", "mod.py")
print(str(SyntaxError("msg", (nested, 3, 1, "x\n", 3, 2))))

print("--- traceback.format_exception_only")
for src in ["x = (1,\n2 +", "1 = 2", "if 1:\n\tpass\n        pass\n", "x = 0777"]:
    try:
        compile(src, "<test>", "exec")
    except SyntaxError as exc:
        print("".join(traceback.format_exception_only(type(exc), exc)), end="")

print("--- import")
# A parser error in a file-backed module reads `text` back from the file (so
# the unclosed bracket keeps its newline); tokenizer errors do not.
sys.dont_write_bytecode = True
tmp = tempfile.mkdtemp(prefix="pyrust_3402_")
sys.path.insert(0, tmp)
modules = {
    "_syntax_attrs_3402_unclosed": "x = 1\ny = (2,\n  3\n",
    "_syntax_attrs_3402_generic": "x = 1\ny = 2 +\n",
    "_syntax_attrs_3402_number": "x = 0777\n",
    "_syntax_attrs_3402_unindent": "if 1:\n    x = 1\n  y = 2\n",
}
try:
    for name, source in modules.items():
        with open(os.path.join(tmp, name + ".py"), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(source)
    importlib.invalidate_caches()
    for name in modules:
        try:
            importlib.import_module(name)
        except SyntaxError as exc:
            print(name, type(exc).__name__)
            print("  ", os.path.basename(exc.filename), exc.lineno, exc.offset,
                  ascii(exc.text), exc.end_lineno, exc.end_offset)
            if not name.endswith("_generic"):
                print("  ", str(exc))
        else:
            print(name, "imported")
finally:
    sys.path.remove(tmp)
    shutil.rmtree(tmp, ignore_errors=True)
