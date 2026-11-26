//! CPython 3.12 docstrings for the `contextlib` module and classes (taken
//! from `Lib/contextlib.py`), applied when a module generation is prepared.

pub(super) const MODULE_DOC: &str = "Utilities for with-statement contexts.  See PEP 343.";

pub(super) const CLASS_DOCS: &[(&str, &str)] = &[
    (
        "AbstractContextManager",
        "An abstract base class for context managers.",
    ),
    (
        "AbstractAsyncContextManager",
        "An abstract base class for asynchronous context managers.",
    ),
    (
        "ContextDecorator",
        "A base class or mixin that enables context managers to work as decorators.",
    ),
    (
        "AsyncContextDecorator",
        "A base class or mixin that enables async context managers to work as decorators.",
    ),
    (
        "_GeneratorContextManagerBase",
        "Shared functionality for @contextmanager and @asynccontextmanager.",
    ),
    (
        "_GeneratorContextManager",
        "Helper for @contextmanager decorator.",
    ),
    (
        "_AsyncGeneratorContextManager",
        "Helper for @asynccontextmanager decorator.",
    ),
    (
        "closing",
        "Context to automatically close something at the end of a block.\n\n    Code like this:\n\n        with closing(<module>.open(<arguments>)) as f:\n            <block>\n\n    is equivalent to this:\n\n        f = <module>.open(<arguments>)\n        try:\n            <block>\n        finally:\n            f.close()\n\n    ",
    ),
    (
        "aclosing",
        "Async context manager for safely finalizing an asynchronously cleaned-up\n    resource such as an async generator, calling its ``aclose()`` method.\n\n    Code like this:\n\n        async with aclosing(<module>.fetch(<arguments>)) as agen:\n            <block>\n\n    is equivalent to this:\n\n        agen = <module>.fetch(<arguments>)\n        try:\n            <block>\n        finally:\n            await agen.aclose()\n\n    ",
    ),
    (
        "suppress",
        "Context manager to suppress specified exceptions\n\n    After the exception is suppressed, execution proceeds with the next\n    statement following the with statement.\n\n         with suppress(FileNotFoundError):\n             os.remove(somefile)\n         # Execution still resumes here if the file was already removed\n    ",
    ),
    (
        "redirect_stdout",
        "Context manager for temporarily redirecting stdout to another file.\n\n        # How to send help() to stderr\n        with redirect_stdout(sys.stderr):\n            help(dir)\n\n        # How to write help() to a file\n        with open('help.txt', 'w') as f:\n            with redirect_stdout(f):\n                help(pow)\n    ",
    ),
    (
        "redirect_stderr",
        "Context manager for temporarily redirecting stderr to another file.",
    ),
    (
        "_BaseExitStack",
        "A base class for ExitStack and AsyncExitStack.",
    ),
    (
        "ExitStack",
        "Context manager for dynamic management of a stack of exit callbacks.\n\n    For example:\n        with ExitStack() as stack:\n            files = [stack.enter_context(open(fname)) for fname in filenames]\n            # All opened files will automatically be closed at the end of\n            # the with statement, even if attempts to open files later\n            # in the list raise an exception.\n    ",
    ),
    (
        "AsyncExitStack",
        "Async context manager for dynamic management of a stack of exit\n    callbacks.\n\n    For example:\n        async with AsyncExitStack() as stack:\n            connections = [await stack.enter_async_context(get_connection())\n                for i in range(5)]\n            # All opened connections will automatically be released at the\n            # end of the async with statement, even if attempts to open a\n            # connection later in the list raise an exception.\n    ",
    ),
    (
        "nullcontext",
        "Context manager that does no additional processing.\n\n    Used as a stand-in for a normal context manager, when a particular\n    block of code is only sometimes used with a normal context manager:\n\n    cm = optional_cm if condition else nullcontext()\n    with cm:\n        # Perform operation, using optional_cm if condition is True\n    ",
    ),
    (
        "chdir",
        "Non thread-safe context manager to change the current working directory.",
    ),
];
