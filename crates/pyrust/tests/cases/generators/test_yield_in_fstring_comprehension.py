# A `yield` reachable only inside an f-string interpolation must still be
# detected by the comprehension generator-classification scan, matching
# CPython's `'yield' inside <kind>` SyntaxError (issue #2313). This mirrors the
# identical f-string gap fixed for `await` in #2308.
#
# Using `compile(...)` so the SyntaxError is caught and its `.msg` printed,
# keeping the output deterministic across CPython and pyrust.

cases = [
    # yield inside an f-string interpolation in a generator expression.
    'g = (f"{(yield x)}" for x in xs)',
    # yield inside an f-string interpolation in a list comprehension.
    'r = [f"{(yield x)}" for x in xs]',
    # yield inside an f-string interpolation in a set comprehension.
    'r = {f"{(yield x)}" for x in xs}',
    # yield inside an f-string interpolation in a dict comprehension value.
    'r = {x: f"{(yield x)}" for x in xs}',
    # yield inside a NESTED format spec interpolation.
    'r = [f"{x:{(yield 1)}}" for x in xs]',
    # plain yield directly in a genexp (control — must also be rejected).
    "g = ((yield x) for x in xs)",
]

for src in cases:
    try:
        compile(src, "<test>", "exec")
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg)

# An f-string interpolation without a yield must still compile cleanly.
compile('g = (f"{x}" for x in xs)', "<test>", "exec")
print("plain f-string genexp compiles")

# A `yield` inside an f-string in a real function body classifies the function
# as a generator (this path already worked; guards against regressions).
def gen():
    x = f"{(yield 5)}"


g = gen()
print("gen type:", type(g).__name__)
print("first yield:", next(g))
