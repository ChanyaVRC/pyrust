# CPython 3.12 parity tests for `shlex` module (issue #3147).
import io
import shlex

# Module surface
print("all:", shlex.__all__)
print("class module:", shlex.shlex.__module__)

# shlex.split — POSIX mode (default)
print("split basic:", shlex.split("this is a test"))
print("split quotes:", shlex.split("""this is "a test" 'of quotes'"""))
print("split empty quoted:", shlex.split('"" \'\' a""b'))
print("split adjacent:", shlex.split("foo'bar'\"baz\""))
print("split escape outside:", shlex.split(r"a\ b c\\d \'e\'"))
print("split escape in dquote:", shlex.split(r'"a\"b\\c\nd"'))
print("split escape in squote:", shlex.split(r"'a\nb\\c'"))
print("split empty:", shlex.split(""))
print("split whitespace only:", shlex.split("   \t\r\n  "))

# shlex.split — comments flag
print("split comments=False:", shlex.split("foo # not a comment", comments=False))
print("split comments=True:", shlex.split("foo # is a comment\nbar # another", comments=True))

# shlex.split — non-POSIX mode
print("split non-posix:", shlex.split('a "b c" \'d e\'', posix=False))
print("split non-posix escape:", shlex.split(r'a\ b "c\"d"', posix=False))

# shlex.split errors
try:
    shlex.split(None)
except ValueError as e:
    print("split None:", str(e))

try:
    shlex.split('unclosed "quote')
except ValueError as e:
    print("split unclosed quote:", str(e))

try:
    shlex.split("trailing\\")
except ValueError as e:
    print("split trailing escape:", str(e))

# shlex.quote
quote_cases = [
    "",
    "hello",
    "foo_bar-123.txt",
    "/usr/local/bin/python3",
    "a@b:c=d+e,f%g",
    "hello world",
    "it's",
    "'single'",
    '"double"',
    "a;b|c&d$e*f",
    "line1\nline2",
    "café",
    "a'b'c",
]
for s in quote_cases:
    q = shlex.quote(s)
    print("quote", repr(s), "->", repr(q))

try:
    shlex.quote(123)
except TypeError:
    print("quote non-str TypeError ok")

# shlex.join & round-trip with shlex.split
join_cases = [
    [],
    ["echo", "hello", "world"],
    ["echo", "-n", "Multiple words", "it's", "", "a;b"],
    ["cmd", "line1\nline2", "'quoted'", '"double"'],
]
for args in join_cases:
    joined = shlex.join(args)
    print("join", args, "->", repr(joined))
    print("roundtrip ok:", shlex.split(joined) == args)

# shlex.shlex class — non-POSIX default
lex1 = shlex.shlex("a = 'hello' # comment\nb = 42")
print("lex1 eof:", repr(lex1.eof), "posix:", lex1.posix, "punct:", repr(lex1.punctuation_chars))
print("lex1 iter is self:", iter(lex1) is lex1)
print("lex1 tokens:", list(lex1))
print("lex1 lineno:", lex1.lineno)

# shlex.shlex class — POSIX mode
lex2 = shlex.shlex("a = 'hello world' # comment\nb = 42", posix=True)
print("lex2 eof:", repr(lex2.eof))
tokens2 = []
while True:
    tok = lex2.get_token()
    if tok == lex2.eof:
        break
    tokens2.append(tok)
print("lex2 tokens:", tokens2)

# push_token
lex3 = shlex.shlex("one two three", posix=True)
print("first:", lex3.get_token())
lex3.push_token("pushed_a")
lex3.push_token("pushed_b")
print("rest:", list(lex3))

# push_source / pop_source with str and StringIO
lex4 = shlex.shlex("outer1 outer2", infile="outer.txt", posix=True)
print("outer1:", lex4.get_token(), lex4.error_leader())
lex4.push_source("inner1\ninner2", newfile="inner.txt")
print("inner1:", lex4.get_token(), lex4.error_leader())
print("inner2:", lex4.get_token(), lex4.error_leader())
print("after inner exhausted:", lex4.get_token(), lex4.error_leader())
print("end:", lex4.get_token())

lex5 = shlex.shlex(io.StringIO("x y"), posix=True)
lex5.push_source(io.StringIO("p q"))
print("StringIO sources:", list(lex5))

# punctuation_chars=True
lex6 = shlex.shlex("a && b; c && d || e; f >'abc'; (def \"ghi\")", posix=True, punctuation_chars=True)
print("punct True:", repr(lex6.punctuation_chars), list(lex6))

# punctuation_chars custom string
lex7 = shlex.shlex("a=1;b=2|c&&d", posix=True, punctuation_chars=";|")
print("punct custom:", repr(lex7.punctuation_chars), list(lex7))

# Custom commenters / whitespace_split
lex8 = shlex.shlex("a-b % comment\nc/d", posix=True)
lex8.commenters = "%"
lex8.whitespace_split = True
print("custom commenters+ws_split:", list(lex8))

print("shlex ok")
