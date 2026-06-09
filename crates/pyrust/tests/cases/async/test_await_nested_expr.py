# Regression: an `await` nested inside a larger expression (e.g. a call
# argument, `print(await x)`) must not corrupt register allocation for a
# *subsequent* await in the same coroutine.  The await lowering allocates the
# result register first and frees its scratch temps in strict LIFO order; an
# earlier version freed them out of order (silent no-ops in the LIFO allocator),
# leaking slots and making the next await raise a spurious
# "TypeError: object is not iterable".

import asyncio


async def leaf():
    return 0


async def boom():
    await leaf()
    raise ValueError("kb")


# await in a call argument, then a second await (the trigger).
async def case_callarg():
    print(await leaf())
    return await leaf()


# await in a call argument, then await a coroutine that itself awaits + returns.
async def case_inner():
    print(await leaf())

    async def mid():
        await leaf()
        return 7

    return await mid()


# await in a call argument, then await a coroutine that awaits + raises, caught.
async def case_raise():
    print(await leaf())
    try:
        await boom()
    except ValueError as e:
        print("caught", e)
    return "done"


# await as an operand of a binary expression, then another await.
async def case_binop():
    x = (await leaf()) + 10
    y = await leaf()
    return x + y


# several awaits nested in nested calls.
async def case_nested_calls():
    return str(int(await leaf())) + str(await leaf())


print(asyncio.run(case_callarg()))
print(asyncio.run(case_inner()))
print(asyncio.run(case_raise()))
print(asyncio.run(case_binop()))
print(asyncio.run(case_nested_calls()))
