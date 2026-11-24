# Parity fixture for #3143: unparenthesized `for` / `async for` iterable tuples
# (`star_expressions` in Python 3.12 grammar) and comprehension `for` boundaries.


def expect_syntax_error(src, label):
    try:
        compile(src, "<test>", "exec")
        raise AssertionError(f"expected SyntaxError for {label}: {src!r}")
    except SyntaxError:
        print("SyntaxError OK:", label)


# --- Basic unparenthesized tuples in `for` ---
out = []
for x in 1, 2, 3:
    out.append(x)
print("basic:", out)

# Single-element trailing comma
out = []
a = 42
for x in a,:
    out.append(x)
print("single trailing:", out)

# Multi-element trailing comma
out = []
for x in 10, 20,:
    out.append(x)
print("multi trailing:", out)

# --- Starred elements in unparenthesized `for` iterable ---
xs = [1, 2]
ys = (3, 4)

out = []
for x in *xs, 99:
    out.append(x)
print("star prefix:", out)

out = []
for x in 0, *xs:
    out.append(x)
print("star suffix:", out)

out = []
for x in *xs,:
    out.append(x)
print("star single:", out)

out = []
for x in *xs, *ys, 5:
    out.append(x)
print("multi star:", out)

# --- Tuple/starred targets combined with unparenthesized tuple iterables ---
pairs = []
for k, v in (1, "a"), (2, "b"):
    pairs.append((k, v))
print("tuple target + tuple iter:", pairs)

unpacked = []
for head, *tail in [1, 2, 3], [4, 5]:
    unpacked.append((head, tail))
print("starred target + tuple iter:", unpacked)

# --- Complex expressions inside unparenthesized tuple iterables ---
flag = True
out = []
for x in ("yes" if flag else "no"), 1 + 2:
    out.append(x)
print("ternary in tuple:", out)

out = []
for x in (captured := [7, 8]):
    out.append(x)
print("paren walrus iter:", out, captured)

out = []
for x in 1, (captured2 := 2):
    out.append(x)
print("paren walrus in tuple:", out, captured2)

# --- `for ... else` with unparenthesized tuple ---
else_ran = False
for x in 1, 2:
    pass
else:
    else_ran = True
print("for-else:", else_ran)

# --- `async for` with unparenthesized tuple iterable ---
class AsyncIter:
    def __init__(self, items):
        self._it = iter(items)

    def __aiter__(self):
        return self

    async def __anext__(self):
        try:
            return next(self._it)
        except StopIteration:
            raise StopAsyncIteration


async def run_async_for():
    r1 = []
    try:
        async for x in 1, 2:
            r1.append(x)
    except TypeError:
        r1.append("TypeError")

    # Verify parsing & evaluation of `async for` iter tuple with real async iter
    # via custom async generator expression / __aiter__ check: a tuple itself is
    # not an async iterable so TypeError proves the tuple was constructed and
    # passed to `__aiter__`.
    r2 = []
    try:
        async for x in *xs, 3:
            r2.append(x)
    except TypeError:
        r2.append("TypeError")

    r3 = []
    async for x in AsyncIter([10, 20]):
        r3.append(x)

    return r1, r2, r3


coro = run_async_for()
try:
    coro.send(None)
except StopIteration as stop:
    print("async for:", stop.value)

# --- SyntaxError parity cases ---
expect_syntax_error("for x in *a: pass", "bare starred for iter")
expect_syntax_error("async def f():\n    async for x in *a: pass", "bare starred async for iter")
expect_syntax_error("for x in a := [1]: pass", "unparenthesized walrus in for iter")
expect_syntax_error("for x in 1, a := 2: pass", "unparenthesized walrus in for tuple")
expect_syntax_error("[x for x in 1, 2]", "unparenthesized tuple in listcomp")
expect_syntax_error("[x for x in 1,]", "trailing comma in listcomp")
expect_syntax_error("(x for x in 1, 2)", "unparenthesized tuple in genexpr")
expect_syntax_error("{x for x in 1, 2}", "unparenthesized tuple in setcomp")
expect_syntax_error("{x: x for x in 1, 2}", "unparenthesized tuple in dictcomp")

print("for iter tuple OK")
