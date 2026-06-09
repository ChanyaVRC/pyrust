# `yield from` is not allowed inside an `async def` body (issue #2280): CPython
# raises SyntaxError, while a bare `yield` is fine (it makes the function an
# async generator).  Using `compile(...)` so the SyntaxError is caught and its
# `.msg` printed, keeping the output deterministic across CPython and pyrust.

cases = [
    # `yield from` directly in an async def.
    "async def f():\n    yield from [1, 2, 3]",
    # `yield from` in an expression position inside an async def.
    "async def f():\n    x = yield from gen()",
    # `return <value>` inside an async generator (an `async def` with a bare
    # `yield`) is a SyntaxError; only a bare `return` is allowed.
    "async def f():\n    yield 1\n    return 5",
    # The yield can appear *after* the offending return — still an async gen.
    "async def f():\n    return 5\n    yield 1",
    # Even a literal `return None` is rejected (any return value is illegal).
    "async def f():\n    yield 1\n    return None",
]

for src in cases:
    try:
        compile(src, "<test>", "exec")
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg)

# A bare `yield` inside an async def is valid (an async generator).
compile("async def f():\n    yield 1", "<test>", "exec")
print("bare yield in async def compiles")
