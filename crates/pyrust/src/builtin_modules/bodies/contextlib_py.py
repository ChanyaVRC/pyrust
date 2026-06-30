"""Pure-Python contextlib members injected onto the native `contextlib` module.

The native Rust body (`contextlib.rs`) provides `suppress`, `contextmanager`,
`closing`, `nullcontext`, `redirect_stdout`, `redirect_stderr`, and `ExitStack`.
This source supplies the remaining CPython members that are most naturally
written in Python (issue #2795):

- `AbstractContextManager` / `AbstractAsyncContextManager` — the ABCs that back
  the sync/async context-manager protocols, with structural `__subclasshook__`.
- `ContextDecorator` / `AsyncContextDecorator` — mixins that let a context
  manager double as a (sync/async) function decorator.
- `asynccontextmanager` — async analogue of `@contextmanager`.
- `aclosing` — async analogue of `closing`.
- `AsyncExitStack` — async analogue of `ExitStack` (handles both sync and async
  context managers).

Mirrors CPython 3.12's `Lib/contextlib.py`.

`ABC`, `abstractmethod` (from `abc`), `wraps` (from `functools`) and
`GenericAlias` (from `types`) are pre-seeded into the exec namespace by
`inject_python_members`, so this source does not import them itself.
"""


def _check_methods(C, *methods):
    mro = C.__mro__
    for method in methods:
        for B in mro:
            if method in B.__dict__:
                if B.__dict__[method] is None:
                    return NotImplemented
                break
        else:
            return NotImplemented
    return True


class AbstractContextManager(ABC):
    """An abstract base class for context managers."""

    __class_getitem__ = classmethod(lambda cls, item: GenericAlias(cls, item))

    def __enter__(self):
        """Return `self` upon entering the runtime context."""
        return self

    @abstractmethod
    def __exit__(self, exc_type, exc_value, traceback):
        """Raise any exception triggered within the runtime context."""
        return None

    @classmethod
    def __subclasshook__(cls, C):
        if cls is AbstractContextManager:
            return _check_methods(C, "__enter__", "__exit__")
        return NotImplemented


class AbstractAsyncContextManager(ABC):
    """An abstract base class for asynchronous context managers."""

    __class_getitem__ = classmethod(lambda cls, item: GenericAlias(cls, item))

    async def __aenter__(self):
        """Return `self` upon entering the runtime context."""
        return self

    @abstractmethod
    async def __aexit__(self, exc_type, exc_value, traceback):
        """Raise any exception triggered within the runtime context."""
        return None

    @classmethod
    def __subclasshook__(cls, C):
        if cls is AbstractAsyncContextManager:
            return _check_methods(C, "__aenter__", "__aexit__")
        return NotImplemented


class ContextDecorator(object):
    """A base class or mixin that enables context managers to work as decorators."""

    def _recreate_cm(self):
        """Return a recreated instance of self.

        Allows an otherwise one-shot context manager like
        _GeneratorContextManager to support use as a decorator via implicit
        recreation.
        """
        return self

    def __call__(self, func):
        @wraps(func)
        def inner(*args, **kwds):
            with self._recreate_cm():
                return func(*args, **kwds)
        return inner


class AsyncContextDecorator(object):
    """A base class or mixin that enables async context managers to work as decorators."""

    def _recreate_cm(self):
        """Return a recreated instance of self."""
        return self

    def __call__(self, func):
        @wraps(func)
        async def inner(*args, **kwds):
            async with self._recreate_cm():
                return await func(*args, **kwds)
        return inner


class _AsyncGeneratorContextManager(AbstractAsyncContextManager, AsyncContextDecorator):
    """Helper for @asynccontextmanager decorator."""

    def __init__(self, func, args, kwds):
        self.gen = func(*args, **kwds)
        self.func, self.args, self.kwds = func, args, kwds
        doc = getattr(func, "__doc__", None)
        if doc is None:
            doc = type(self).__doc__
        self.__doc__ = doc

    def _recreate_cm(self):
        return self.__class__(self.func, self.args, self.kwds)

    async def __aenter__(self):
        del self.args, self.kwds, self.func
        try:
            return await self.gen.__anext__()
        except StopAsyncIteration:
            raise RuntimeError("generator didn't yield") from None

    async def __aexit__(self, typ, value, traceback):
        if typ is None:
            try:
                await self.gen.__anext__()
            except StopAsyncIteration:
                return False
            else:
                raise RuntimeError("generator didn't stop")
        else:
            if value is None:
                value = typ()
            try:
                await self.gen.athrow(value)
            except StopAsyncIteration as exc:
                return exc is not value
            except RuntimeError as exc:
                if exc is value:
                    exc.__traceback__ = traceback
                    return False
                if (
                    isinstance(value, StopIteration)
                    and exc.__cause__ is value
                ):
                    value.__traceback__ = traceback
                    return False
                raise
            except BaseException as exc:
                if exc is not value:
                    raise
                exc.__traceback__ = traceback
                return False
            raise RuntimeError("generator didn't stop after athrow()")


def asynccontextmanager(func):
    """@asynccontextmanager decorator.

    Typical usage:

        @asynccontextmanager
        async def some_async_generator(<arguments>):
            <setup>
            try:
                yield <value>
            finally:
                <cleanup>

    This makes this:

        async with some_async_generator(<arguments>) as <variable>:
            <body>

    equivalent to this:

        <setup>
        try:
            <variable> = <value>
            <body>
        finally:
            <cleanup>
    """
    @wraps(func)
    def helper(*args, **kwds):
        return _AsyncGeneratorContextManager(func, args, kwds)
    return helper


class aclosing(AbstractAsyncContextManager):
    """Async context manager for safely finalizing an asynchronously cleaned-up
    resource such as an async generator, calling its `aclose()` method.

    Code like this:

        async with aclosing(<module>.fetch(<arguments>)) as agen:
            <block>

    is equivalent to this:

        agen = <module>.fetch(<arguments>)
        try:
            <block>
        finally:
            await agen.aclose()

    """

    def __init__(self, thing):
        self.thing = thing

    async def __aenter__(self):
        return self.thing

    async def __aexit__(self, *exc_info):
        await self.thing.aclose()


class AsyncExitStack(AbstractAsyncContextManager):
    """Async context manager for dynamic management of a stack of exit
    callbacks.

    For example:
        async with AsyncExitStack() as stack:
            connections = [await stack.enter_async_context(get_connection())
                for i in range(5)]
            # All opened connections will automatically be released at the
            # end of the async with statement, even if attempts to open a
            # connection later in the list raise an exception.
    """

    def __init__(self):
        self._exit_callbacks = []

    def pop_all(self):
        """Preserve the context stack by transferring it to a new instance."""
        new_stack = type(self)()
        new_stack._exit_callbacks = self._exit_callbacks
        self._exit_callbacks = []
        return new_stack

    def push_async_callback(self, callback, /, *args, **kwds):
        """Registers an arbitrary coroutine function and arguments.

        Cannot suppress exceptions.
        """
        async def _exit_wrapper(exc_type, exc, tb):
            await callback(*args, **kwds)

        self._push_cm_exit(_exit_wrapper, True)
        return callback

    def callback(self, callback, /, *args, **kwds):
        """Registers an arbitrary callback and arguments.

        Cannot suppress exceptions.
        """
        def _exit_wrapper(exc_type, exc, tb):
            callback(*args, **kwds)

        self._push_cm_exit(_exit_wrapper, False)
        return callback

    def push(self, exit):
        """Registers a callback with the standard __exit__ method signature.

        Can suppress exceptions the same way __exit__ method can.
        Also accepts any object with an __exit__ method (registering a call
        to the method instead of the object itself).
        """
        try:
            exit_method = type(exit).__exit__
        except AttributeError:
            self._push_cm_exit(exit, False)
        else:
            obj = exit
            self._push_cm_exit(lambda *a: exit_method(obj, *a), False)
        return exit

    def push_async_exit(self, exit):
        """Registers a coroutine function with the standard __aexit__ method
        signature.

        Can suppress exceptions the same way __aexit__ method can.
        Also accepts any object with an __aexit__ method (registering a call
        to the method instead of the object itself).
        """
        try:
            exit_method = type(exit).__aexit__
        except AttributeError:
            self._push_cm_exit(exit, True)
        else:
            obj = exit

            async def _wrap(*a):
                return await exit_method(obj, *a)

            self._push_cm_exit(_wrap, True)
        return exit

    def enter_context(self, cm):
        """Enters the supplied context manager.

        If successful, also pushes its __exit__ method as a callback and
        returns the result of the __enter__ method.
        """
        cls = type(cm)
        try:
            _enter = cls.__enter__
            _exit = cls.__exit__
        except AttributeError:
            raise TypeError(
                f"'{cls.__module__}.{cls.__qualname__}' object does "
                f"not support the context manager protocol"
            ) from None
        result = _enter(cm)
        self._push_cm_exit(lambda *a: _exit(cm, *a), False)
        return result

    async def enter_async_context(self, cm):
        """Enters the supplied async context manager.

        If successful, also pushes its __aexit__ method as a callback and
        returns the result of the __aenter__ method.
        """
        cls = type(cm)
        try:
            _enter = cls.__aenter__
            _exit = cls.__aexit__
        except AttributeError:
            raise TypeError(
                f"'{cls.__module__}.{cls.__qualname__}' object does "
                f"not support the asynchronous context manager protocol"
            ) from None
        result = await _enter(cm)

        async def _wrap(*a):
            return await _exit(cm, *a)

        self._push_cm_exit(_wrap, True)
        return result

    def _push_cm_exit(self, cm_exit, is_async):
        """Helper to correctly register callbacks to __(a)exit__ methods."""
        self._exit_callbacks.append((is_async, cm_exit))

    async def aclose(self):
        """Immediately unwind the context stack."""
        await self.__aexit__(None, None, None)

    async def __aenter__(self):
        return self

    async def __aexit__(self, *exc_details):
        received_exc = exc_details[0] is not None

        # We manipulate the exception state so it behaves as though
        # we were actually nesting multiple with statements
        frame_exc = exc_details[1]

        suppressed_exc = False
        pending_raise = False
        while self._exit_callbacks:
            is_async, cb = self._exit_callbacks.pop()
            try:
                if is_async:
                    cb_suppress = await cb(*exc_details)
                else:
                    cb_suppress = cb(*exc_details)

                if cb_suppress:
                    suppressed_exc = True
                    pending_raise = False
                    exc_details = (None, None, None)
            except BaseException as new_exc:
                pending_raise = True
                exc_details = (type(new_exc), new_exc, None)
                frame_exc = new_exc

        if pending_raise:
            raise frame_exc

        return received_exc and suppressed_exc
