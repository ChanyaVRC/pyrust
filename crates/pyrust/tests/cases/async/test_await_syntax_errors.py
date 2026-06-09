# `await` is only valid inside an `async def` (issue #1039).
#
# Using `compile(...)` so the SyntaxError is caught and its `.msg` printed,
# keeping the output deterministic across CPython and pyrust.

cases = [
    # await inside a plain (non-async) function → "outside async function"
    "def f():\n    await 5",
    # await at module scope → "outside function"
    "await 5",
    # await inside a nested non-async function (the async enclosing scope does
    # not make a nested plain def async).
    "async def f():\n    def g():\n        await 5",
]

for src in cases:
    try:
        compile(src, "<test>", "exec")
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg)

# Valid await inside async def compiles cleanly.
compile("async def f():\n    await g()", "<test>", "exec")
print("valid await compiles")
