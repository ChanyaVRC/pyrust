# Closure / enclosing-scope capture inside f-string interpolations.
#
# Regression coverage for issue #444: every AST recursor (closure-capture
# analyser, free-var collector, walrus walker, ...) must walk the
# sub-expressions embedded in an f-string, including those inside a format
# spec (e.g. `f"{x:>{width}}"`).  Names referenced inside `{...}` must be
# promoted to cell vars just like any other free-var read.

# ---------------------------------------------------------------------------
# Parameter named `f` (the canonical workaround case from PR #415 / #381).
# ---------------------------------------------------------------------------
def make(f):
    def inner():
        return f"{f(1)}"
    return inner

doubler = lambda x: x * 2
assert make(doubler)() == "2", make(doubler)()


# ---------------------------------------------------------------------------
# Nested capture inside an f-string body.
# ---------------------------------------------------------------------------
def adder(n):
    def fmt(v):
        return f"{n}+{v}={n+v}"
    return fmt

assert adder(10)(5) == "10+5=15", adder(10)(5)


# ---------------------------------------------------------------------------
# Capture through two levels of nesting (the outermost binds, the middle is
# transparent, the innermost reads inside an f-string).
# ---------------------------------------------------------------------------
def outer():
    msg = "hello"
    def middle():
        def inner():
            return f"{msg}!"
        return inner()
    return middle()

assert outer() == "hello!", outer()


# ---------------------------------------------------------------------------
# Multiple distinct captures referenced in one f-string.
# ---------------------------------------------------------------------------
def make2(prefix, suffix):
    def fmt(x):
        return f"{prefix}{x}{suffix}"
    return fmt

assert make2("[", "]")("hi") == "[hi]", make2("[", "]")("hi")


# ---------------------------------------------------------------------------
# Captured name referenced inside a format spec — the spec is itself a mini
# f-string, so its scope-pass must descend into the nested `{width}`.
# ---------------------------------------------------------------------------
def fmt_with_width(width):
    def fmt(x):
        return f"{x:>{width}}"
    return fmt

assert fmt_with_width(10)("hi") == "        hi", repr(fmt_with_width(10)("hi"))


# ---------------------------------------------------------------------------
# Conversion + captured spec — `!r` plus a captured precision.
# ---------------------------------------------------------------------------
def fmt_with_precision(prec):
    def fmt(x):
        return f"{x:.{prec}f}"
    return fmt

assert fmt_with_precision(2)(3.14159) == "3.14", fmt_with_precision(2)(3.14159)
assert fmt_with_precision(4)(3.14159) == "3.1416", fmt_with_precision(4)(3.14159)


# ---------------------------------------------------------------------------
# Walrus inside an f-string interpolation binds in the enclosing scope.
# (Compiled bytecode path — exercised when the file is executed as a script.)
# ---------------------------------------------------------------------------
def walrus_in_fstring():
    s = f"{(n := 10)}"
    assert n == 10, n
    return s

assert walrus_in_fstring() == "10", walrus_in_fstring()


# ---------------------------------------------------------------------------
# Decorator-style parameter named `f` — the original repro from the PR that
# motivated this fix (the f-string in `wrap` references the captured `f`).
# ---------------------------------------------------------------------------
def trace(name):
    def deco(f):
        def wrap(x):
            result = f(x)
            return f"trace-{name}/{result}"
        return wrap
    return deco

@trace("outer")
def identity(x):
    return x

assert identity(7) == "trace-outer/7", identity(7)


# ---------------------------------------------------------------------------
# Lambda capture inside an f-string — the closure-capture analyser walks
# Expr::Lambda { body } and the body is an FString that reads the captured
# name.
# ---------------------------------------------------------------------------
def make_lambda(prefix):
    return lambda x: f"{prefix}:{x}"

assert make_lambda("tag")("v") == "tag:v", make_lambda("tag")("v")


# ---------------------------------------------------------------------------
# Both the main expr and the format spec capture different enclosing names.
# ---------------------------------------------------------------------------
def make_padded(value, width):
    def fmt():
        return f"{value:>{width}}"
    return fmt

assert make_padded("ok", 6)() == "    ok", repr(make_padded("ok", 6)())


print("fstring capture OK")
