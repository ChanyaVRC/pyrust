"""
PEP 617: parenthesized `with` statement (Python 3.10+).
"""


class CM:
    def __init__(self, n):
        self.n = n

    def __enter__(self):
        return self.n

    def __exit__(self, *a):
        return False


# Two CMs with as-bindings inside parens — the primary PEP 617 form.
with (CM(1) as a, CM(2) as b):
    print(a, b)

# Trailing comma is allowed.
with (CM(3) as c, CM(4) as d,):
    print(c, d)

# Single CM in parens with as-binding.
with (CM(5) as e):
    print(e)

# Single CM in parens with trailing comma (PEP 617 single-item form).
with (CM(6) as f,):
    print(f)

# Multiple CMs with no as-binding (PEP 617, no optional_vars).
with (CM(7), CM(8)):
    print("no-as")

# Non-parenthesized multi-CM form must still work (regression guard).
with CM(9) as g, CM(10) as h:
    print(g, h)

# Parenthesized single CM with no as-binding and no comma — plain paren expr.
with (CM(11)):
    print("paren-expr")

# Paren group followed by `as` at outer level → tuple-as-CM expression.
try:
    with (CM(12), CM(13)) as pair:
        pass
except (TypeError, AttributeError):
    # CPython 3.10+ raises TypeError; older pyrust may raise AttributeError.
    # Either way: the statement parses without a ParseError.
    print("tuple-cm-error")
