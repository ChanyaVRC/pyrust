# CPython 3.12 parity test for issue #1816.
# Lexer errors for unknown characters should produce "invalid syntax",
# matching CPython's normalised SyntaxError message.

# Unknown character '?' raises SyntaxError with message "invalid syntax"
try:
    compile("???", "<test>", "eval")
except SyntaxError as e:
    print(repr(e.msg))

# Another unknown character '$' (not a valid Python token)
try:
    compile("$x", "<test>", "eval")
except SyntaxError as e:
    print(repr(e.msg))

# Valid syntax still parses without error
result = compile("1 + 2", "<test>", "eval")
print("valid ok")

# Multi-character valid expressions still work
result = compile("x + y", "<test>", "eval")
print("valid expr ok")
