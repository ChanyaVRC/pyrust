"""Offline deterministic CPython 3.12 parity tests for all 26 public
`asyncio.*` submodules, class hierarchies, and helper utilities (issue #3254).
"""

import asyncio
import asyncio.base_events
import asyncio.base_futures
import asyncio.base_subprocess
import asyncio.base_tasks
import asyncio.constants
import asyncio.coroutines
import asyncio.events
import asyncio.exceptions
import asyncio.format_helpers
import asyncio.futures
import asyncio.locks
import asyncio.log
import asyncio.mixins
import asyncio.protocols
import asyncio.queues
import asyncio.runners
import asyncio.selector_events
import asyncio.staggered
import asyncio.streams
import asyncio.subprocess
import asyncio.taskgroups
import asyncio.tasks
import asyncio.threads
import asyncio.timeouts
import asyncio.transports
import asyncio.trsock
import logging


# ---------------------------------------------------------------------------
# 1. Submodule imports & exact object identity with top-level `asyncio`
# ---------------------------------------------------------------------------
assert asyncio.exceptions.CancelledError is asyncio.CancelledError
assert asyncio.exceptions.InvalidStateError is asyncio.InvalidStateError
assert asyncio.exceptions.BrokenBarrierError is asyncio.BrokenBarrierError
assert asyncio.exceptions.TimeoutError is asyncio.TimeoutError is TimeoutError
assert asyncio.exceptions.IncompleteReadError is asyncio.IncompleteReadError
assert asyncio.exceptions.LimitOverrunError is asyncio.LimitOverrunError
assert asyncio.exceptions.SendfileNotAvailableError is asyncio.SendfileNotAvailableError

assert asyncio.streams.StreamReader is asyncio.StreamReader
assert asyncio.streams.StreamWriter is asyncio.StreamWriter
assert asyncio.streams.StreamReaderProtocol is asyncio.StreamReaderProtocol
assert asyncio.streams.open_connection is asyncio.open_connection
assert asyncio.streams.start_server is asyncio.start_server
assert asyncio.streams._DEFAULT_LIMIT == 65536

assert asyncio.tasks.Task is asyncio.Task
assert asyncio.tasks.create_task is asyncio.create_task
assert asyncio.tasks.ensure_future is asyncio.ensure_future
assert asyncio.tasks.gather is asyncio.gather
assert asyncio.tasks.sleep is asyncio.sleep
assert asyncio.tasks.wait is asyncio.wait
assert asyncio.tasks.wait_for is asyncio.wait_for
assert asyncio.tasks.as_completed is asyncio.as_completed
assert asyncio.tasks.shield is asyncio.shield
assert asyncio.tasks.current_task is asyncio.current_task
assert asyncio.tasks.all_tasks is asyncio.all_tasks
assert asyncio.tasks.eager_task_factory is asyncio.eager_task_factory
assert asyncio.tasks.create_eager_task_factory is asyncio.create_eager_task_factory

assert asyncio.futures.Future is asyncio.Future
assert asyncio.futures.wrap_future is asyncio.wrap_future
assert asyncio.futures.isfuture is asyncio.isfuture

assert asyncio.locks.Lock is asyncio.Lock
assert asyncio.locks.Event is asyncio.Event
assert asyncio.locks.Condition is asyncio.Condition
assert asyncio.locks.Semaphore is asyncio.Semaphore
assert asyncio.locks.BoundedSemaphore is asyncio.BoundedSemaphore
assert asyncio.locks.Barrier is asyncio.Barrier

assert asyncio.queues.Queue is asyncio.Queue
assert asyncio.queues.LifoQueue is asyncio.LifoQueue
assert asyncio.queues.PriorityQueue is asyncio.PriorityQueue
assert asyncio.queues.QueueEmpty is asyncio.QueueEmpty
assert asyncio.queues.QueueFull is asyncio.QueueFull

assert asyncio.runners.Runner is asyncio.Runner
assert asyncio.runners.run is asyncio.run

assert asyncio.events.AbstractEventLoop is asyncio.AbstractEventLoop
assert asyncio.events.AbstractEventLoopPolicy is asyncio.AbstractEventLoopPolicy
assert asyncio.events.AbstractServer is asyncio.AbstractServer
assert asyncio.events.Handle is asyncio.Handle
assert asyncio.events.TimerHandle is asyncio.TimerHandle
assert asyncio.events.get_event_loop_policy is asyncio.get_event_loop_policy
assert asyncio.events.set_event_loop_policy is asyncio.set_event_loop_policy
assert asyncio.events.new_event_loop is asyncio.new_event_loop

assert asyncio.protocols.BaseProtocol is asyncio.BaseProtocol
assert asyncio.protocols.Protocol is asyncio.Protocol
assert asyncio.protocols.BufferedProtocol is asyncio.BufferedProtocol
assert asyncio.protocols.DatagramProtocol is asyncio.DatagramProtocol
assert asyncio.protocols.SubprocessProtocol is asyncio.SubprocessProtocol

assert asyncio.transports.BaseTransport is asyncio.BaseTransport
assert asyncio.transports.ReadTransport is asyncio.ReadTransport
assert asyncio.transports.WriteTransport is asyncio.WriteTransport
assert asyncio.transports.Transport is asyncio.Transport
assert asyncio.transports.DatagramTransport is asyncio.DatagramTransport
assert asyncio.transports.SubprocessTransport is asyncio.SubprocessTransport

assert asyncio.subprocess.create_subprocess_exec is asyncio.create_subprocess_exec
assert asyncio.subprocess.create_subprocess_shell is asyncio.create_subprocess_shell
assert asyncio.subprocess.PIPE == -1
assert asyncio.subprocess.STDOUT == -2
assert asyncio.subprocess.DEVNULL == -3
assert hasattr(asyncio.subprocess, "Process")
assert hasattr(asyncio.subprocess, "SubprocessStreamProtocol")

assert asyncio.timeouts.Timeout is asyncio.Timeout
assert asyncio.timeouts.timeout is asyncio.timeout
assert asyncio.timeouts.timeout_at is asyncio.timeout_at

assert asyncio.taskgroups.TaskGroup is asyncio.TaskGroup
assert asyncio.coroutines.iscoroutine is asyncio.iscoroutine
assert asyncio.coroutines.iscoroutinefunction is asyncio.iscoroutinefunction
assert asyncio.threads.to_thread is asyncio.to_thread
assert asyncio.base_events.BaseEventLoop is asyncio.BaseEventLoop
assert asyncio.base_events.Server is asyncio.Server
assert hasattr(asyncio.selector_events, "BaseSelectorEventLoop")
assert hasattr(asyncio.trsock, "TransportSocket")
assert hasattr(asyncio.mixins, "_LoopBoundMixin")
assert hasattr(asyncio.base_futures, "_PENDING")
assert hasattr(asyncio.base_futures, "_CANCELLED")
assert hasattr(asyncio.base_futures, "_FINISHED")
assert hasattr(asyncio.base_tasks, "_task_repr_info")
assert hasattr(asyncio.base_subprocess, "BaseSubprocessTransport")
print("submodule_identities: ok")


# ---------------------------------------------------------------------------
# 2. Class hierarchies
# ---------------------------------------------------------------------------
assert issubclass(asyncio.Transport, asyncio.ReadTransport)
assert issubclass(asyncio.Transport, asyncio.WriteTransport)
assert issubclass(asyncio.ReadTransport, asyncio.BaseTransport)
assert issubclass(asyncio.WriteTransport, asyncio.BaseTransport)
assert issubclass(asyncio.DatagramTransport, asyncio.BaseTransport)
assert issubclass(asyncio.SubprocessTransport, asyncio.BaseTransport)

assert issubclass(asyncio.Protocol, asyncio.BaseProtocol)
assert issubclass(asyncio.BufferedProtocol, asyncio.BaseProtocol)
assert issubclass(asyncio.DatagramProtocol, asyncio.BaseProtocol)
assert issubclass(asyncio.SubprocessProtocol, asyncio.BaseProtocol)
assert issubclass(asyncio.StreamReaderProtocol, asyncio.Protocol)

assert issubclass(asyncio.TimerHandle, asyncio.Handle)
assert issubclass(asyncio.SelectorEventLoop, asyncio.BaseEventLoop)
assert issubclass(asyncio.BaseEventLoop, asyncio.AbstractEventLoop)
assert issubclass(asyncio.DefaultEventLoopPolicy, asyncio.AbstractEventLoopPolicy)
assert issubclass(asyncio.Server, asyncio.AbstractServer)

assert issubclass(asyncio.IncompleteReadError, EOFError)
assert issubclass(asyncio.LimitOverrunError, Exception)
assert issubclass(asyncio.SendfileNotAvailableError, RuntimeError)
print("hierarchies: ok")


# ---------------------------------------------------------------------------
# 3. Coroutine predicates, constants, log, format_helpers, staggered_race
# ---------------------------------------------------------------------------
async def sample_coro():
    return 1


def regular_fn():
    return 1


assert asyncio.iscoroutinefunction(sample_coro) is True
assert asyncio.iscoroutinefunction(regular_fn) is False
assert asyncio.iscoroutinefunction(asyncio.sleep) is True
assert asyncio.iscoroutinefunction(asyncio.open_connection) is True
assert asyncio.iscoroutinefunction(asyncio.to_thread) is True

c = sample_coro()
assert asyncio.iscoroutine(c) is True
assert asyncio.iscoroutine(123) is False
c.close()

assert asyncio.constants.LOG_THRESHOLD_FOR_CONNLOST_WRITES == 5
assert asyncio.constants.ACCEPT_RETRY_DELAY == 1
assert asyncio.constants.DEBUG_STACK_DEPTH == 10
assert asyncio.constants.SSL_HANDSHAKE_TIMEOUT == 60.0
assert asyncio.constants.SSL_SHUTDOWN_TIMEOUT == 30.0
assert asyncio.constants.SENDFILE_FALLBACK_READBUFFER_SIZE == 262144
assert asyncio.constants.THREAD_JOIN_TIMEOUT == 300

assert isinstance(asyncio.log.logger, logging.Logger)
assert asyncio.log.logger.name == "asyncio"

fmt = asyncio.format_helpers._format_args_and_kwargs((1, "a"), {"x": 2})
assert fmt == "(1, 'a', x=2)"

# Policy & manual loop lifecycle
policy = asyncio.get_event_loop_policy()
assert isinstance(policy, asyncio.AbstractEventLoopPolicy)
lp = asyncio.new_event_loop()
assert isinstance(lp, asyncio.BaseEventLoop)
assert not lp.is_running() and not lp.is_closed()
assert lp.run_until_complete(sample_coro()) == 1
lp.close()
assert lp.is_closed()


async def test_staggered():
    async def slow():
        await asyncio.sleep(0.05)
        return "slow"

    async def fast():
        await asyncio.sleep(0.005)
        return "fast"

    winner, idx, excs = await asyncio.staggered.staggered_race(
        [slow, fast], delay=0.01
    )
    assert winner == "fast" and idx == 1 and len(excs) == 2
    print("staggered: ok")


asyncio.run(test_staggered())
print("all_submodules: ok")
