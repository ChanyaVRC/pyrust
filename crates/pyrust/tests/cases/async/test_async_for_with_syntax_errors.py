# `async for` / `async with` are only valid inside an `async def` (issue #2279).
#
# Using `compile(...)` so the SyntaxError is caught and its `.msg` printed,
# keeping the output deterministic across CPython and pyrust.

cases = [
    # async for in a plain function.
    "def f():\n    async for x in y:\n        pass",
    # async for at module scope.
    "async for x in y:\n    pass",
    # async with in a plain function.
    "def f():\n    async with c as v:\n        pass",
    # async with at module scope.
    "async with c as v:\n    pass",
    # nested plain def inside async def does not inherit async-ness.
    "async def f():\n    def g():\n        async for x in y:\n            pass",
]

for src in cases:
    try:
        compile(src, "<test>", "exec")
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg)

# Valid inside async def compiles cleanly.
compile("async def f():\n    async for x in y:\n        pass", "<test>", "exec")
compile("async def f():\n    async with c as v:\n        pass", "<test>", "exec")
print("valid async for/with compiles")
