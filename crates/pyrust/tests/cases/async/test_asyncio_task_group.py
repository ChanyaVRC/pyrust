# asyncio.TaskGroup parity tests (Python 3.11/3.12, issue #3154).

import asyncio


async def make_val(v, delay=0):
    await asyncio.sleep(delay)
    return v


async def test_basic_task_group():
    async with asyncio.TaskGroup() as tg:
        t1 = tg.create_task(make_val(10), name="ten")
        t2 = tg.create_task(make_val(20, 0.01), name="twenty")
        print("inside names:", t1.get_name(), t2.get_name())
    print("basic results:", t1.result(), t2.result(), t1.done(), t2.done())


async def test_sibling_cancellation_and_except_star():
    sibling_cancelled = False

    async def failing():
        await asyncio.sleep(0.01)
        raise ValueError("bad value")

    async def slow_sibling():
        nonlocal sibling_cancelled
        try:
            await asyncio.sleep(10)
        except asyncio.CancelledError:
            sibling_cancelled = True
            raise

    try:
        async with asyncio.TaskGroup() as tg:
            t1 = tg.create_task(failing())
            t2 = tg.create_task(slow_sibling())
    except* ValueError as eg:
        print(
            "caught EG:",
            type(eg).__name__,
            eg.message,
            [str(e) for e in eg.exceptions],
        )
    print("sibling cancelled:", sibling_cancelled, t2.cancelled(), t1.done())


async def test_multiple_errors():
    ev = asyncio.Event()

    async def fail_value():
        await ev.wait()
        raise ValueError("v")

    async def fail_type_on_cancel():
        try:
            ev.set()
            await asyncio.sleep(10)
        except asyncio.CancelledError:
            raise TypeError("t")

    caught = []
    try:
        async with asyncio.TaskGroup() as tg:
            tg.create_task(fail_value())
            tg.create_task(fail_type_on_cancel())
    except* ValueError as eg:
        caught.append(("V", [str(e) for e in eg.exceptions]))
    except* TypeError as eg:
        caught.append(("T", [str(e) for e in eg.exceptions]))
    print("multi errors:", caught)


async def test_body_exception_cancels_children():
    child_cancelled = False

    async def child():
        nonlocal child_cancelled
        try:
            await asyncio.sleep(10)
        except asyncio.CancelledError:
            child_cancelled = True
            raise

    try:
        async with asyncio.TaskGroup() as tg:
            tg.create_task(child())
            await asyncio.sleep(0)
            raise KeyError("body")
    except* KeyError as eg:
        print("body error:", [type(e).__name__ for e in eg.exceptions])
    print("body child cancelled:", child_cancelled)


async def test_base_exception_group():
    class MyBase(BaseException):
        pass

    async def fail_base():
        await asyncio.sleep(0)
        raise MyBase("custom base")

    try:
        async with asyncio.TaskGroup() as tg:
            tg.create_task(fail_base())
    except BaseExceptionGroup as eg:
        print(
            "base eg:",
            type(eg).__name__,
            isinstance(eg, BaseExceptionGroup),
            isinstance(eg, ExceptionGroup),
            len(eg.exceptions),
            type(eg.exceptions[0]).__name__,
        )


async def test_nested_task_groups():
    outer_cancelled = False

    async def inner_fail():
        await asyncio.sleep(0.01)
        raise RuntimeError("deep")

    async def outer_waiter():
        nonlocal outer_cancelled
        try:
            await asyncio.sleep(10)
        except asyncio.CancelledError:
            outer_cancelled = True
            raise

    async def run_inner():
        async with asyncio.TaskGroup() as inner:
            inner.create_task(inner_fail())

    try:
        async with asyncio.TaskGroup() as outer:
            outer.create_task(outer_waiter())
            outer.create_task(run_inner())
    except* RuntimeError as eg:
        print("nested caught:", type(eg).__name__, len(eg.exceptions))
    print("nested outer cancelled:", outer_cancelled)


async def test_outer_cancellation_propagates():
    child_cancelled = False

    async def child():
        nonlocal child_cancelled
        try:
            await asyncio.sleep(10)
        except asyncio.CancelledError:
            child_cancelled = True
            raise

    async def runner():
        async with asyncio.TaskGroup() as tg:
            tg.create_task(child())

    t = asyncio.create_task(runner())
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    t.cancel()
    try:
        await t
    except asyncio.CancelledError:
        print("outer cancel -> CancelledError:", t.cancelled())
    print("outer cancel child cancelled:", child_cancelled)


async def test_invalid_states():
    tg = asyncio.TaskGroup()
    c = make_val(1)
    try:
        tg.create_task(c)
    except RuntimeError as e:
        print("before enter:", "has not been entered" in str(e))
    finally:
        c.close()

    async with tg:
        try:
            async with tg:
                pass
        except RuntimeError as e:
            print("re-enter:", "already been entered" in str(e))

    c2 = make_val(2)
    try:
        tg.create_task(c2)
    except RuntimeError as e:
        print("after finish:", "is finished" in str(e))
    finally:
        c2.close()


async def main():
    await test_basic_task_group()
    await test_sibling_cancellation_and_except_star()
    await test_multiple_errors()
    await test_body_exception_cancels_children()
    await test_base_exception_group()
    await test_nested_task_groups()
    await test_outer_cancellation_propagates()
    await test_invalid_states()


asyncio.run(main())
