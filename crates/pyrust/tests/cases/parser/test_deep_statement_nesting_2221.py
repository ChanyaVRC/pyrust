# Issue #2221: deeply nested statements must raise a catchable exception
# (CPython 3.12 raises IndentationError: too many levels of indentation when
# the indentation stack exceeds MAXINDENT == 100 levels) instead of overflowing
# the parser's native stack and aborting with SIGABRT.


def build(n):
    lines = []
    for i in range(n):
        lines.append("    " * i + "if True:")
    lines.append("    " * n + "pass")
    return "\n".join(lines) + "\n"


# 99 nested levels are accepted (the base column-0 level plus 99 indents).
exec(build(99))
print("99 levels: ok")

# The 100th level is rejected with a catchable IndentationError.
for n in (100, 5000):
    try:
        exec(build(n))
        print(n, "levels: unexpectedly ok")
    except IndentationError as e:
        print(n, "levels:", type(e).__name__)

# IndentationError is a subclass of SyntaxError, so a SyntaxError handler also
# catches it — and the failure is an ordinary catchable Python exception.
try:
    exec(build(2000))
except SyntaxError as e:
    print("caught as SyntaxError subclass:", isinstance(e, IndentationError))
