# `compile()` raises `IndentationError` (a `SyntaxError` subclass) with CPython
# 3.12's wording and location for a missing indented block, an unexpected
# indent and an inconsistent unindent (issue #3403).  Filenames are pseudo
# names so CPython never reads `text` back from a file on disk.


def attrs(exc):
    return (exc.filename, exc.lineno, exc.offset, exc.text, exc.end_lineno, exc.end_offset)


def full(label, src, mode="exec"):
    try:
        compile(src, "<test>", mode)
    except SyntaxError as exc:
        print(label, type(exc).__name__, ascii(exc.args[0]), ascii(attrs(exc)))
    else:
        print(label, "no error")


print("--- issue reproducer")
for src in [
    "if 1:\npass",
    "  x = 1",
    "if 1:\n  x = 1\n y = 2",
    "def f():\nreturn 1",
    "class C:\npass",
    "for i in [1]:\npass",
    "while 0:\npass",
    "try:\npass",
    "with x:\npass",
]:
    try:
        compile(src, "s.py", "exec")
    except SyntaxError as e:
        print(type(e).__name__, e.msg, e.lineno, e.offset)

print("--- hierarchy")
try:
    compile("if 1:\npass", "<test>", "exec")
except SyntaxError as exc:
    print(isinstance(exc, IndentationError), type(exc) is IndentationError)
try:
    compile("if 1:\npass", "<test>", "exec")
except IndentationError as exc:
    print("caught as IndentationError")

print("--- missing block after each statement kind")
full("if", "if 1:\npass")
full("if-eof", "if 1:\n")
full("if-eof-nonl", "if 1:")
full("if-comment-eof", "if 1:\n# c\n")
full("if-blank-then-stmt", "if 1:\n\n\nx = 1")
full("elif", "if 1:\n  pass\nelif 2:\npass")
full("else", "if 1:\n  pass\nelse:\npass")
full("else-eof", "if 1:\n  pass\nelse:\n")
full("for", "for i in x:\npass")
full("for-else", "for i in x:\n  pass\nelse:\npass")
full("async-for", "async def f():\n  async for i in x:\n  pass")
full("while", "while 0:\npass")
full("while-else", "while 0:\n  pass\nelse:\nx = 1")
full("def", "def f():\nreturn 1")
full("def-args", "def f(a, b=1, *c, **d):\nreturn 1")
full("def-annotated", "def f(a: int) -> int:\nreturn a")
full("def-decorated", "@d\n@e\ndef f():\nreturn 1")
full("async-def", "async def f():\npass")
full("class", "class C:\npass")
full("class-bases", "class C(A, metaclass=M):\npass")
full("class-decorated", "@d\nclass C:\npass")
full("with", "with x:\npass")
full("with-as", "with x as y, z as w:\npass")
full("with-parens", "with (x as y, z):\npass")
full("async-with", "async def f():\n  async with x:\n  pass")
full("try", "try:\npass")
full("try-eof", "try:\n")
full("except", "try:\n  pass\nexcept E:\npass")
full("except-as", "try:\n  pass\nexcept E as e:\npass")
full("except-bare", "try:\n  pass\nexcept:\npass")
full("except-star", "try:\n  pass\nexcept* E:\npass")
full("try-else", "try:\n  pass\nexcept E:\n  pass\nelse:\npass")
full("finally", "try:\n  pass\nfinally:\npass")
full("finally-eof", "try:\n  pass\nfinally:\n")
full("match", "match x:\npass")
full("match-call", "match(x):\npass")
full("case", "match x:\n  case 1:\n  pass")
full("case-guard", "match x:\n  case 1 if y:\n  pass")
full("case-eof", "match x:\n  case 1:\n")

print("--- missing block location variants")
full("nested-if", "def f():\n  if 1:\n  x = 1")
full("nested-dedent", "def f():\n  if 1:\nx = 1")
full("nested-dedent-2", "class C:\n  def f(self):\n    if x:\n  y = 1")
full("nested-eof", "def f():\n  if 1:\n")
full("nested-eof-comment", "def f():\n  if 1:\n  # c\n")
full("header-after-semicolon", "x = 1; y = 2\nif x:\nz = 3")
full("header-line-3", "x = 1\ny = 2\nwhile x:\nz = 3")
full("crlf", "if 1:\r\npass\r\n")
full("tab-body", "if 1:\n\tpass\nif 2:\npass")
full("trailing-text", "if 1:\npass  # trailing")
full("compound-body", "if 1:\nif 2:\n  pass")
full("async-def-nested", "class C:\n  async def f(self):\n  pass")

print("--- unexpected indent")
full("indent-first", "  x = 1")
full("indent-tab", "\tx = 1")
full("indent-after-stmt", "x = 1\n  y = 2")
full("indent-in-block", "if 1:\n  x = 1\n    y = 2")
full("indent-in-def", "def f():\n  x = 1\n    y = 2\n  z = 3")
full("indent-after-compound", "if 1:\n  x = 1\nelse:\n  y = 2\n    z = 3")
full("indent-comment-then-stmt", "# c\n  x = 1")
full("indent-def", "x = 1\n  def f():\n    pass")
full("indent-keyword", "  pass")
full("indent-expr", "  1 + 2")
full("indent-eval", "  1 + 2", "eval")
full("indent-eval-line2", "1\n  2", "eval")
full("indent-single", "  x = 1\n", "single")
full("indent-single-2", "x = 1\n  y = 2\n", "single")
full("indent-after-class", "class C:\n  x = 1\n    y = 2")
full("indent-line-4", "a = 1\nb = 2\nc = 3\n   d = 4\n")

print("--- unindent does not match")
full("unindent-simple", "if 1:\n  x = 1\n y = 2")
full("unindent-deep", "if 1:\n    if 2:\n        x = 1\n      y = 2")
full("unindent-def", "def f():\n    x = 1\n  y = 2")
full("unindent-tab", "if 1:\n\tx = 1\n  y = 2")
full("unindent-eof", "if 1:\n    x = 1\n  y = 2\n")
full("unindent-class", "class C:\n    def f(self):\n        pass\n      x = 1")

print("--- valid layouts compile")
for src in [
    "if 1:\n  pass",
    "if 1:\n\n  # c\n  pass",
    "if 1: pass",
    "if 1:\n  pass\nelse:\n  pass",
    "def f():\n  if 1:\n    pass\n  x = 1",
    "x = 1\n\ny = 2\n",
    "# c\n# d\nx = 1",
]:
    full("ok", src)

print("--- other errors are not IndentationError")
for src in ["x = = 1", "if 1\n  pass", "else:\n  pass", "x = (\n1"]:
    try:
        compile(src, "<test>", "exec")
    except SyntaxError as exc:
        print(type(exc).__name__, isinstance(exc, IndentationError))

print("--- exec / eval / single")
for src, modes in [
    ("if 1:\npass", ("exec", "eval", "single")),
    ("  x = 1", ("exec", "eval", "single")),
    ("if 1:\n  x = 1\n y = 2", ("exec", "single")),
]:
    for mode in modes:
        full("mode-" + mode, src, mode)

print("--- exec()/eval() of the same sources")
for src in ["if 1:\npass", "  x = 1"]:
    for fn in (exec, eval):
        try:
            fn(src)
        except SyntaxError as exc:
            print(fn.__name__, type(exc).__name__, ascii(exc.args[0]), ascii(attrs(exc)))
try:
    exec("if 1:\n  x = 1\n y = 2")
except SyntaxError as exc:
    print("exec", type(exc).__name__, ascii(exc.args[0]), ascii(attrs(exc)))

print("--- message / str / repr")
try:
    compile("if 1:\npass", "f.py", "exec")
except IndentationError as exc:
    print(str(exc))
    print(repr(exc))
    print(exc.msg, exc.lineno, exc.offset, ascii(exc.text), exc.filename)
    print(exc.print_file_and_line)
    print(exc.args[0], exc.args[1])
