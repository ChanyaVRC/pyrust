# Parity fixture for #3374: in a raw f-string (`rf"..."`, `fr"..."`, any case)
# a backslash never escapes a following `{` or `}` -- it is an ordinary
# literal character, and the brace keeps its f-string meaning (replacement
# field start, or half of a `{{` / `}}` pair).  Every line must stay
# byte-for-byte with CPython 3.12.
import ast
import warnings

warnings.simplefilter("ignore")

x = 5
y = "s"


class Echo:
    def __format__(self, spec):
        return "<" + spec + ">"


e = Echo()

# ── the reproducer from the issue ────────────────────────────────────────────
print(repr(rf"\{x}\n"))
print(repr(fr"a\{x+1}\b"))
print(repr(rf"\\{x}"))
print(repr(rf"""\{x}\n"""))
print(repr(rf"\{{literal\}}"))

# ── every prefix spelling ────────────────────────────────────────────────────
print(repr(rf"\{x}"), repr(fr"\{x}"), repr(RF"\{x}"), repr(FR"\{x}"))
print(repr(rF"\{x}"), repr(Rf"\{x}"), repr(fR"\{x}"), repr(Fr"\{x}"))
print(repr(rf'\{x}'), repr(rf'''\{x}'''), repr(rf"""\{x}"""))

# ── backslash runs before a field ────────────────────────────────────────────
print(repr(rf"\{x}"), repr(rf"\\{x}"), repr(rf"\\\{x}"), repr(rf"\\\\{x}"))
print(repr(rf"a\{x}b"), repr(rf"a\\{x}b"), repr(rf"{x}\{x}\\{x}\\\{x}"))
print(repr(rf"\{x}\{x}"), repr(rf"\{x}\n\t\{x}\r"))

# ── backslash before `}` and the `{{` / `}}` escapes ─────────────────────────
print(repr(rf"\}}"), repr(rf"\{{"), repr(rf"\{{x}}"), repr(rf"\{{{x}}}"))
print(repr(rf"\\}}"), repr(rf"\\{{"), repr(rf"{{\}}"), repr(rf"{{\{x}\}}"))
print(repr(rf"\{{literal\}}"), repr(rf"\{{{x}\}}"))

# ── format specs, conversions, debug `=` ─────────────────────────────────────
print(repr(rf"\{x:d}"), repr(rf"\{x:>4}"), repr(rf"\{x:04d}"), repr(rf"a\{x:^5}b"))
print(repr(rf"\{y!r}"), repr(rf"\{y!s}"), repr(rf"\{'é'!a}"))
print(repr(rf"\{x=}"), repr(rf"\{x = }"), repr(rf"\{y=!r}"), repr(rf"\{x=:>4}"))
print(repr(rf"\{x:{x}}"), repr(rf"\{x:>{x}}"), repr(rf"\{x:{'>'}{x}}"))
# a backslash inside a format spec is literal in a raw f-string; one before a
# nested field must not swallow the field either.
print(repr(rf"{e:\{x}}"), repr(rf"{e:\\{x}}"), repr(rf"{e:a\{x}b}"))
print(repr(rf"{e:\n}"), repr(rf"{e:\{x}\n}"))

# ── nested quotes / expressions ──────────────────────────────────────────────
print(repr(rf"\{'a'}"), repr(rf'\{"a"}'), repr(rf"\{x + 1}"), repr(rf"\{[1, 2]}"))
print(repr(rf"\{ {'k': 1}['k'] }"), repr(rf"\{(lambda: 3)()}"))
print(repr(rf"\{f'{x}'}"), repr(rf"\{rf'\{x}'}"), repr(rf'\{rf"\{x}"}'))

# ── triple quotes, multi-line ────────────────────────────────────────────────
print(repr(rf"""\{x}"""), repr(rf'''a\{x}b'''))
print(repr(rf"""\{x}
\{x}
\\{x}
"""))
print(repr(rf"""\{x}\"""" + '"'))
print(repr(fr'''\{y!r}\{x:3}\{x=}'''))
print(repr(rf"""q"\{x}"q"""))

# ── a backslash before the closing quote still escapes it ────────────────────
print(repr(rf"\""), repr(rf"\{x}\""), repr(rf'\'{x}\''), repr(rf"\\"))

# ── non-raw f-strings: `\{` is an invalid escape, the brace stays a field ────
print(repr(f"\{x}"), repr(f"\\{x}"), repr(f"\\\{x}"), repr(f"a\{x}b\n"))
print(repr(f"\{x:d}"), repr(f"\{y!r}"), repr(f"\{x=}"))
print(repr(f"\{{x}}"), repr(f"\}}"), repr(f"""\{x}"""))

# ── runtime: the compiled value, not just the lexer ──────────────────────────
for i in range(3):
    print(rf"\{i}", rf"\\{i}", rf"{i}\{i}")
print(len(rf"\{x}"), len(rf"\\{x}"), len(rf"\{x}\n"))
print(rf"\{x}" == "\\5", rf"\\{x}" == "\\\\5", rf"\{x}\n" == "\\5\\n")

# ── AST shape: backslash is a Constant char, the field a FormattedValue ──────
for src in (
    'rf"\\{x}"',
    'rf"\\{x}\\n"',
    'fr"a\\{x+1}\\b"',
    'rf"\\\\{x}"',
    'rf"\\{{lit\\}}"',
    'rf"\\{x:d}"',
    'rf"\\{x!r}"',
    'rf"\\{x=}"',
    'rf"""\\{x}\\n"""',
    'f"\\{x}"',
    'f"\\\\{x}"',
):
    tree = ast.parse(src)
    print(src, "->", ast.dump(tree.body[0].value), "|", ast.unparse(tree))
    # round-trip: the unparsed source must evaluate to the same string
    print("  roundtrip:", eval(ast.unparse(tree)) == eval(src))

# ── compile-time evaluation through eval() of the same sources ───────────────
for src in (
    'rf"\\{x}\\n"',
    'rf"\\{x:>{x}}"',
    'rf"\\{y!r}\\{x=}"',
    'f"\\{x}\\\\{x}"',
):
    print(src, "=>", repr(eval(src)))

# ── error cases must still be errors (type only for the unterminated /
# lone-brace forms: their wording is a separate, pre-existing gap) ───────────
for bad in ('rf"\\{"', 'rf"\\}"', 'rf"\\{x"', 'f"\\{"', 'f"\\}"'):
    try:
        eval(bad)
    except SyntaxError:
        print(bad, "-> SyntaxError")
for bad in ('rf"\\{}"', 'rf"\\{x!z}"'):
    try:
        eval(bad)
    except SyntaxError as err:
        print(bad, "-> SyntaxError", err.msg)

# ── CRLF / CR line endings inside a multi-line f-string become "\n" ──────────
for nl in ("\r\n", "\r"):
    src = 'rf"""\\{x}' + nl + '\\\\{x}' + nl + '"""'
    print(repr(eval(src)))
    src = 'f"""a' + nl + '{x}' + nl + '"""'
    print(repr(eval(src)))
