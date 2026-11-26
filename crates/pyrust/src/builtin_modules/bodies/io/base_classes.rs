//! Abstract base classes of the `io` module (`IOBase`, `RawIOBase`,
//! `BufferedIOBase`, `TextIOBase`) and the module-level wiring that is not
//! expressible inside `pyrust_module!`.
//!
//! The classes themselves are declared natively in `io.rs`; this file owns the
//! CPython 3.12 docstrings and [`prepare_module`], which finalizes the module
//! right after it is built (see `finalization.rs::prepare_builtin_module`).

use crate::interpreter::Interpreter;
use crate::value::{Value, ValueKind};

pub(super) const IOBASE_DOC: &str = "The abstract base class for all I/O classes.\n\
\n\
This class provides dummy implementations for many methods that\n\
derived classes can override selectively; the default implementations\n\
represent a file that cannot be read, written or seeked.\n\
\n\
Even though IOBase does not declare read, readinto, or write because\n\
their signatures will vary, implementations and clients should\n\
consider those methods part of the interface. Also, implementations\n\
may raise UnsupportedOperation when operations they do not support are\n\
called.\n\
\n\
The basic type used for binary data read from or written to a file is\n\
bytes. Other bytes-like objects are accepted as method arguments too.\n\
In some cases (such as readinto), a writable object is required. Text\n\
I/O classes work with str data.\n\
\n\
Note that calling any method (except additional calls to close(),\n\
which are ignored) on a closed stream should raise a ValueError.\n\
\n\
IOBase (and its subclasses) support the iterator protocol, meaning\n\
that an IOBase object can be iterated over yielding the lines in a\n\
stream.\n\
\n\
IOBase also supports the :keyword:`with` statement. In this example,\n\
fp is closed after the suite of the with statement is complete:\n\
\n\
with open('spam.txt', 'r') as fp:\n\
\x20   fp.write('Spam and eggs!')\n";

pub(super) const RAWIOBASE_DOC: &str = "Base class for raw binary I/O.";

pub(super) const BUFFEREDIOBASE_DOC: &str = "Base class for buffered IO objects.\n\
\n\
The main difference with RawIOBase is that the read() method\n\
supports omitting the size argument, and does not have a default\n\
implementation that defers to readinto().\n\
\n\
In addition, read(), readinto() and write() may raise\n\
BlockingIOError if the underlying raw stream is in non-blocking\n\
mode and not ready; unlike their raw counterparts, they will never\n\
return None.\n\
\n\
A typical implementation should not inherit from a RawIOBase\n\
implementation, but wrap one.\n";

pub(super) const TEXTIOBASE_DOC: &str = "Base class for text I/O.\n\
\n\
This class provides a character and line based interface to stream\n\
I/O. There is no readinto method because Python's character strings\n\
are immutable.\n";

/// Finalize the freshly built `io` module (issue #2778 / #3342):
///
/// * `UnsupportedOperation` is published from the exception registry and
///   `closed` becomes a property on the concrete streams,
/// * the abstract base classes report `__module__ == "io"` like CPython's
///   `io.py` re-exports,
/// * the concrete in-memory streams keep CPython's C-level `_io` module name
///   (and error-message type name),
/// * `io.open` is the very same object as the built-in `open`.
///
/// The class hierarchy (`BytesIO` → `BufferedIOBase` → `IOBase`, `StringIO` →
/// `TextIOBase` → `IOBase`) is declared in the native `pyrust_module!` block.
pub(super) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    if let Some(exception) = crate::interpreter::lookup_exc_class("io.UnsupportedOperation") {
        module_rc.borrow_mut().insert_attr(
            "UnsupportedOperation".to_string(),
            Value::py_class(exception),
        );
    }
    for class_name in ["StringIO", "BytesIO"] {
        let class = module_rc.borrow().attrs.get(class_name).cloned();
        let Some(ValueKind::PyClass(class)) = class.as_ref().map(Value::kind) else {
            continue;
        };
        let getter = class.borrow().attrs.get("closed").cloned();
        if let Some(getter) = getter {
            let property =
                pyrust_builtins::property::property(getter, Value::none(), Value::none());
            class
                .borrow_mut()
                .attrs
                .insert("closed".to_string(), property);
        }
    }
    for name in ["IOBase", "RawIOBase", "BufferedIOBase", "TextIOBase"] {
        let class = module_rc.borrow().attrs.get(name).cloned();
        if let Some(class) = class
            && let ValueKind::PyClass(class) = class.kind()
        {
            class
                .borrow_mut()
                .attrs
                .insert("__module__".to_string(), Value::string("io"));
        }
    }
    for (name, error_name) in [("BytesIO", "_io.BytesIO"), ("StringIO", "_io.StringIO")] {
        let class = module_rc.borrow().attrs.get(name).cloned();
        if let Some(class) = class
            && let ValueKind::PyClass(class) = class.kind()
        {
            let mut class = class.borrow_mut();
            class.error_name = Some(error_name);
            class
                .attrs
                .insert("__module__".to_string(), Value::string("_io"));
        }
    }
    if let Ok(builtins) = interp.load_module("builtins")
        && let Ok(open) = interp.get_attr(&builtins, "open")
    {
        module_rc.borrow_mut().insert_attr("open".to_string(), open);
    }
}
