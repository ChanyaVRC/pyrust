# Async comprehension syntax (PEP 530).
#
# List, set, and dict comprehensions with `async for` raise SyntaxError
# when they appear outside an async function — both in CPython 3.12 and
# pyrust (the latter does not yet support async iteration at runtime, so
# all async comprehensions are rejected at compile time for now).
#
# Generator expressions with `async for` are permitted at module scope by
# CPython (they create async generator objects), but require __aiter__
# support to iterate; that case is not tested here.

forms = [
    "[x async for x in [1, 2, 3]]",
    "{x async for x in [1, 2, 3]}",
    "{x: x * 2 async for x in [1, 2, 3]}",
]

for form in forms:
    try:
        compile(form, "<test>", "eval")
        print("no error")
    except SyntaxError:
        print("SyntaxError")

# Regular (synchronous) comprehensions must still work correctly.
print([x * 2 for x in range(3)])
print({x for x in range(3)})
print({x: x * 2 for x in range(3)})

# `async def` must parse without error (the body is never called here).
async def placeholder():
    pass

print("async def ok")
