# Parity fixture for #3143: PEP 701 f-string lexer/parser edge cases
# - `# ...` comments inside multi-line f-string `{...}` expressions
# - nested format specifiers (`f"{x:{w:{n}}}"`, `f"{1:{2:{3}}}"`, depth > 2 error)
# - quote reuse, backslashes in expressions, nested multi-line f-strings


def expect_syntax_error(src, label, exact_msg=None):
    try:
        compile(src, "<test>", "eval")
        raise AssertionError(f"expected SyntaxError for {label}: {src!r}")
    except SyntaxError as e:
        msg = str(e).split(" (")[0]
        if exact_msg is not None:
            assert msg == exact_msg, f"{label}: got {msg!r}, expected {exact_msg!r}"
        print("SyntaxError OK:", label)


# --- Comments inside multi-line f-string expressions ---
print("comment brace triple:", repr(f"""{1 # }
}"""))

print("comment specials:", repr(f"""{1 + # } : !r = ' " ( [ {
2}"""))

print("comment single quote:", repr(f"{1 # }
}"))

print("multiline single quote:", repr(f"{1 +
2}"))

print("multiline weird indent:", repr(f"""{
    1 +
  2
}"""))

print("comment before conv:", repr(f"""{1 # }
!r}"""))

print("comment before spec:", repr(f"""{1 # } :
:5}"""))

print("comment after conv:", repr(f"""{1!r # } :
:5}"""))

print("comment after conv close:", repr(f"""{1!r # }
}"""))

# Debug `=` combined with comments (CPython strips comment, keeps whitespace/newline)
print("debug comment before eq:", repr(f"""{1 # } =
=}"""))

print("debug comment after eq:", repr(f"""{1= # }
}"""))

print("debug comment after eq + spec:", repr(f"""{1= # }
:5}"""))

print("debug comment after eq + conv + spec:", repr(f"""{1= # }
!r:5}"""))

# --- Nested format specifiers (up to CPython's depth-2 limit) ---
val = 3.14159
width = 10
prec = 3
print("width.prec:", repr(f"{val:{width}.{prec}f}"))

x = 42
w = 10
n = 2
print("depth2 simple:", repr(f"{x:{w:{n}}}"))
print("depth2 align:", repr(f"{42:{10:>{2}}}"))
print("depth2 innermost literal spec:", repr(f"{1:{10:{2:d}}}"))
print("depth2 innermost empty spec:", repr(f"{1:{10:{2:}}}"))
print("depth2 conv + spec:", repr(f"{x:{w!s:{n}}}"))
print("depth2 with comments:", repr(f"{42:{10 # }
:{2 # }
}}}"))

# `f"{1:{2:{3}}}"` parses cleanly and raises ValueError at runtime because
# format(2, "3") == "  2", which has leading spaces invalid in int's format spec.
try:
    _ = f"{1:{2:{3}}}"
    raise AssertionError("expected ValueError")
except ValueError:
    print("depth2 runtime ValueError OK")

# Nested f-string inside format spec resets format-spec nesting depth to 0.
print("nested fstring resets depth:", repr(f"{42:{f'{10:{2:{1}}}'}}"))

# --- Quote reuse & nested f-strings (PEP 701) ---
print("single in double:", repr(f"{'a'}"))
print("double in single:", repr(f'{"a"}'))
print("same double:", repr(f"{"a"}"))
print("same single:", repr(f'{'a'}'))
print("nested same double:", repr(f"{f"{1}"}"))
print("triple nested same double:", repr(f"{f"{"a"}"}"))
print("deep nested fstring:", repr(f"{f"{f"a"}"}"))
print("nested brace literal same quote:", repr(f"{f"{"}"}"}"))
print("nested mixed quote in single:", repr(f"{f'{"'"}'}"))
print("nested raw fstring:", repr(f"{rf"{1}"}"), repr(f"{fr"{1}"}"))

# Nested multi-line f-strings with comments
print("nested multiline comment:", repr(f"""{f'''{1 # }
}'''}"""))

print("nested same-quote multiline comment:", repr(f"{f"""{1 + # }
2}"""}"))

# --- Backslashes inside expressions ---
print("backslash join:", repr(f"{'\n'.join(['a', 'b'])}"))
print("backslash tab split:", repr(f"{'a\tb'.split('\t')}"))
print("line continuation in expr:", repr(f"{1 + \
2}"))
print("raw string in expr:", repr(f"{r'a\nb'}"))
print("escape in format spec:", repr(f"{42:\x35}"))

# --- Tuple / starred / walrus expressions inside f-string ---
items = [10, 20]
print("tuple in fstring:", repr(f"{1, 2}"), repr(f"{1,}"))
print("starred tuple in fstring:", repr(f"{*items, 30}"))
print("walrus in fstring:", repr(f"{(bound := 99)}"), bound)

# --- SyntaxError parity cases ---
expect_syntax_error(
    'f"{1:{2:{3:{4}}}}"',
    "depth 3 format spec nesting",
    exact_msg="f-string: expressions nested too deeply",
)
expect_syntax_error('f"{1 # }"', "comment eats closing brace")
expect_syntax_error('f"{}"', "empty f-string expr")
expect_syntax_error('f"{   }"', "whitespace-only f-string expr")
expect_syntax_error('f"{1 2}"', "trailing token in f-string expr")
expect_syntax_error('f"{1)}"', "unmatched paren in f-string expr")
expect_syntax_error('f"{*a}"', "bare starred in f-string expr")

print("pep701 fstrings OK")
