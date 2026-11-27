use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use crate::object_model::{
    PyClass, Value, ValueKind, class_chain_contains_builtin_exception, format_unicode_decode_str,
    format_unicode_encode_str,
};

// ─────────────────────────────────────────────────────────────────────────────
// Error types
// ─────────────────────────────────────────────────────────────────────────────

/// Heap-allocated payload of [`PyError`].
///
/// Kept behind `Box<PyErrorKind>` inside [`PyError`] so `PyError` is a single
/// pointer (`8 B`) and `Result<Value, PyError>` stays `16 B` across all
/// fallible VM and runtime call boundaries.
#[derive(Debug, Clone)]
pub enum PyErrorKind {
    Lex(String),
    Parse(String),
    Runtime(String),
    /// A named Python exception identified by **class name string**.
    ///
    /// Used by builtin code in `pyrust-core` and `pyrust-builtins` that
    /// cannot hold a `Rc<RefCell<PyClass>>` because the class objects live
    /// in the interpreter crate.  The VM materialises this into a
    /// `PyInstance` via an env-hierarchy name lookup before propagating.
    ///
    /// `class_name` is a `Cow<'static, str>` so the overwhelmingly common
    /// case (a string literal like `"TypeError"`) is zero-allocation; rare
    /// dynamic class names can still be carried via `Cow::Owned`.
    Named(Cow<'static, str>, String), // (class_name, message)
    /// A named Python exception identified by **class identity** (`Rc`).
    ///
    /// Used by interpreter-internal raise sites that already hold the class
    /// object (e.g. the VM dispatch loop).  The VM can materialise this into
    /// a `PyInstance` directly, with no env-hierarchy name lookup — making
    /// the hot path (typed exception from a built-in opcode) zero-lookup.
    ///
    /// Construct via [`PyError::class`].  Call sites in `pyrust-core` and
    /// `pyrust-builtins` that cannot reference a `PyClass` Rc should
    /// continue to use [`PyError::named`] / `PyErrorKind::Named`.
    Class(Rc<RefCell<PyClass>>, String), // (class, message)
    /// A `KeyError` that carries the **raw key `Value`** as `args[0]`.
    ///
    /// CPython stores the key object itself in `args[0]`, not a stringified
    /// repr.  This variant lets all raise sites (builtin and VM) pass the key
    /// through without pre-rendering it as a string.  The VM materialises it
    /// as `instantiate_exception(KeyError_class, vec![key])`.
    KeyError(Value),
    /// An `ImportError` (or `ModuleNotFoundError`) that carries the module
    /// name so the VM can set `.name` and `.path` on the resulting instance.
    ///
    /// CPython 3.12: `ImportError.__init__` accepts `*args, name=None,
    /// path=None` keyword arguments and stores them as instance attributes.
    /// Raise sites that know the module name should use this variant instead
    /// of `PyErrorKind::Named("ImportError", …)` so that `e.name` works in
    /// `except ImportError as e:` blocks.
    ///
    /// `class_name` is `"ImportError"` or `"ModuleNotFoundError"`.
    /// `module_name` is `None` when the module name is not available.
    ImportError {
        class_name: &'static str,
        message: String,
        module_name: Option<String>,
    },
    /// An `OSError` (or one of its subclasses) raised from an OS-level
    /// operation.  Carries the structured fields that CPython 3.12 sets on
    /// every OS-sourced exception: `errno`, `strerror`, and optionally
    /// `filename` and `filename2`.
    ///
    /// The VM materialises this into a `PyInstance` via
    /// `instantiate_os_error`, which populates `errno`, `strerror`,
    /// `filename`, and `filename2` as instance attributes — matching
    /// CPython's `OSError.__init__(errno, strerror[, filename])` behaviour.
    /// Two-path operations (e.g. `os.rename`) set both `filename` (src) and
    /// `filename2` (dst).
    OsError {
        class_name: &'static str,
        errno: i64,
        strerror: String,
        filename: Option<String>,
        filename2: Option<String>,
    },
    /// A `UnicodeDecodeError` raised from an internal decoding operation (e.g.
    /// `bytes.decode()`).  Carries the five structured fields that CPython 3.12
    /// sets on every `UnicodeDecodeError` instance: `encoding`, `object`,
    /// `start`, `end`, and `reason`.
    ///
    /// The VM materialises this into a `PyInstance` via
    /// `instantiate_unicode_decode_error`, populating all five attributes.
    UnicodeDecodeError {
        encoding: String,
        object: Vec<u8>,
        start: usize,
        end: usize,
        reason: String,
    },
    /// A `UnicodeEncodeError` raised from an internal encoding operation (e.g.
    /// `str.encode()`).  Carries the five structured fields that CPython 3.12
    /// sets on every `UnicodeEncodeError` instance.
    UnicodeEncodeError {
        encoding: String,
        object: String,
        start: usize,
        end: usize,
        reason: String,
    },
    /// A `NameError` (or `UnboundLocalError`) raised by the VM when a name
    /// lookup fails.  Carries the identifier string so the VM can set `.name`
    /// on the resulting instance, matching CPython 3.12 parity.
    ///
    /// CPython 3.12: `NameError.__init__` stores the identifier that was not
    /// found as `self.name`.  The attribute is `None` for user-constructed
    /// instances (`NameError('msg')`) and the identifier string for
    /// interpreter-raised instances.
    ///
    /// `class_name` is `"NameError"` or `"UnboundLocalError"`.
    /// `name` is the identifier that was not found (set as the `.name`
    /// attribute); `None` for `UnboundLocalError` (CPython 3.12 parity).
    NameError {
        class_name: &'static str,
        message: String,
        name: Option<String>,
    },
    /// An `AttributeError` raised by the VM when an attribute lookup fails.
    /// Carries the attribute name and the receiver object so the VM can set
    /// `.name` and `.obj` on the resulting instance, matching CPython 3.12
    /// parity.
    ///
    /// CPython 3.12: `AttributeError.__init__` stores the attribute name as
    /// `self.name` and the object on which the lookup was attempted as
    /// `self.obj`.  User-constructed instances (`AttributeError('msg')`) have
    /// both set to `None`.
    ///
    /// `name` is the attribute that was not found; `None` when not available.
    /// `obj` is the receiver on which the lookup was attempted; `None` when
    /// not available.
    AttributeError {
        message: String,
        name: Option<String>,
        obj: Option<Value>,
    },
    /// A compile-time `SyntaxError` (or its `IndentationError` / `TabError`
    /// subclasses) raised by the lexer or parser with the source location
    /// CPython 3.12 stores in `SyntaxError.args[1]` (issue #3402).  Boxed so
    /// the extra location fields do not grow every other variant.
    Syntax(Box<SyntaxErrorInfo>),
    Raised(Value),
}

/// How the `text` attribute of a lexer/parser syntax error is derived from the
/// source once it is known.  CPython 3.12 builds it differently for tokenizer
/// errors (`_syntaxerror_range`) and parser errors
/// (`_PyPegen_raise_error_known_location`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxTextRule {
    /// Tokenizer error: the error line without its trailing newline.
    TokenizerLine,
    /// Parser error: the error line, keeping its trailing newline only when the
    /// tokenizer had not read past it.  `tokenizer_lineno` is the line the
    /// tokenizer stood on when the error was raised; `0` means it had reached
    /// the end of the input (unclosed-bracket errors).
    ParserLine { tokenizer_lineno: u32 },
}

/// Location data a lexer/parser syntax error carries before it is resolved
/// against its source text and filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxErrorPending {
    /// Absolute char indices into the source being lexed, in CPython's
    /// `tok->cur` convention: `offset` is the 0-based column of `cur` within
    /// its line and `end_offset` the 0-based column of `end_cur`.  The lexer
    /// converts this to lines/offsets before the error leaves it.
    Chars {
        cur: usize,
        end_cur: usize,
        rule: SyntaxTextRule,
    },
    /// `lineno` / `offset` / `end_lineno` / `end_offset` are set; only `text`
    /// remains to be derived with `rule`.  `newline_token` marks an error
    /// anchored on a NEWLINE token: when that is the implicit newline at the
    /// end of `eval`/`single` input (no `'\n'` in the source), CPython reports
    /// `offset == end_offset == 0`.
    Text {
        rule: SyntaxTextRule,
        newline_token: bool,
    },
    /// `eval` / `single` input holding nothing but blank lines and comments.
    /// CPython reports its ENDMARKER: `lineno` is the number of source lines
    /// (`0` for empty input), both offsets are `0`, and `text` is the last
    /// line as written.
    EmptyInput,
}

/// Structured payload of [`PyErrorKind::Syntax`]: the exception class plus the
/// `(filename, lineno, offset, text, end_lineno, end_offset)` fields CPython
/// 3.12 stores on compile-time syntax errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxErrorInfo {
    /// `"SyntaxError"`, `"IndentationError"` or `"TabError"`.
    pub class_name: &'static str,
    pub msg: String,
    pub filename: Option<String>,
    pub lineno: Option<i64>,
    pub offset: Option<i64>,
    pub text: Option<String>,
    pub end_lineno: Option<i64>,
    pub end_offset: Option<i64>,
    /// `Some` while the error still needs its source text / filename.
    pub pending: Option<SyntaxErrorPending>,
}

impl SyntaxErrorInfo {
    /// A `SyntaxError` positioned by absolute char indices into the source
    /// being lexed (see [`SyntaxErrorPending::Chars`]).
    pub fn at_chars(
        msg: impl Into<String>,
        cur: usize,
        end_cur: usize,
        rule: SyntaxTextRule,
    ) -> Self {
        Self {
            class_name: "SyntaxError",
            msg: msg.into(),
            filename: None,
            lineno: None,
            offset: None,
            text: None,
            end_lineno: None,
            end_offset: None,
            pending: Some(SyntaxErrorPending::Chars { cur, end_cur, rule }),
        }
    }

    /// A `SyntaxError` at a known 1-based line and CPython-style offsets whose
    /// `text` is still to be derived from the source with `rule`.
    pub fn at_lines(
        msg: impl Into<String>,
        (lineno, offset): (u32, i64),
        (end_lineno, end_offset): (u32, i64),
        rule: SyntaxTextRule,
        newline_token: bool,
    ) -> Self {
        Self {
            class_name: "SyntaxError",
            msg: msg.into(),
            filename: None,
            lineno: Some(i64::from(lineno)),
            offset: Some(offset),
            text: None,
            end_lineno: Some(i64::from(end_lineno)),
            end_offset: Some(end_offset),
            pending: Some(SyntaxErrorPending::Text {
                rule,
                newline_token,
            }),
        }
    }

    /// A `SyntaxError` for `eval` / `single` input without any statement,
    /// located once the source is known (see
    /// [`SyntaxErrorPending::EmptyInput`]).
    pub fn empty_input(msg: impl Into<String>) -> Self {
        Self {
            class_name: "SyntaxError",
            msg: msg.into(),
            filename: None,
            lineno: None,
            offset: None,
            text: None,
            end_lineno: None,
            end_offset: None,
            pending: Some(SyntaxErrorPending::EmptyInput),
        }
    }
}

/// Compact boxed error type (`8 B`) used across the workspace `Result<T>`.
///
/// The payload lives on the heap in [`PyErrorKind`] so the error branch stays
/// cold and `Result<Value, PyError>` fits in `16 B` (two machine registers).
#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct PyError(Box<PyErrorKind>);

impl From<PyErrorKind> for PyError {
    #[inline]
    fn from(kind: PyErrorKind) -> Self {
        Self::new(kind)
    }
}

// Thread-local justification:
// (a) Why `thread_local!`: the parked box is raw scratch memory on the cold
//     error path; it is recycled without threading an allocator handle
//     through every `Result` return path, and without atomics or locks.
// (b) Multi-interpreter / multi-threaded impact: interpreters on one thread
//     share the single parked box, which only ever holds the empty placeholder
//     payload and therefore carries no Python-observable state; each OS thread
//     owns its own box and frees it on exit.
thread_local! {
    /// One emptied [`PyError`] box per thread, handed back by
    /// [`PyError::into_kind`] and reused by the next [`PyError::new`].
    ///
    /// Raising an exception that a handler then catches boxes one payload
    /// and unboxes it again a few instructions later; keeping that single
    /// allocation alive turns the pair into two moves (issue #3426).  The
    /// parked box only ever holds the empty [`PyErrorKind::Runtime`]
    /// placeholder, never a live exception value.
    static SPARE_ERROR_BOX: std::cell::Cell<Option<Box<PyErrorKind>>> =
        const { std::cell::Cell::new(None) };
}

impl PyError {
    /// Box a [`PyErrorKind`] into a compact [`PyError`].
    #[cold]
    #[inline]
    pub fn new(kind: PyErrorKind) -> Self {
        match SPARE_ERROR_BOX.try_with(std::cell::Cell::take) {
            Ok(Some(mut spare)) => {
                *spare = kind;
                Self(spare)
            }
            _ => Self(Box::new(kind)),
        }
    }

    /// Borrow the underlying [`PyErrorKind`].
    #[inline]
    pub fn kind(&self) -> &PyErrorKind {
        &self.0
    }

    /// Mutably borrow the underlying [`PyErrorKind`].
    #[inline]
    pub fn kind_mut(&mut self) -> &mut PyErrorKind {
        &mut self.0
    }

    /// Unbox and return the underlying [`PyErrorKind`].
    #[inline]
    pub fn into_kind(self) -> PyErrorKind {
        let mut boxed = self.0;
        let kind = std::mem::replace(&mut *boxed, PyErrorKind::Runtime(String::new()));
        // Park the emptied box for the next error.  A thread that is tearing
        // down its thread-locals simply frees it.
        let _ = SPARE_ERROR_BOX.try_with(|spare| spare.set(Some(boxed)));
        kind
    }

    /// Constructor for a lexer error.
    #[cold]
    #[inline]
    pub fn lex(msg: String) -> Self {
        Self::new(PyErrorKind::Lex(msg))
    }

    /// Constructor for a parser error.
    #[cold]
    #[inline]
    pub fn parse(msg: String) -> Self {
        Self::new(PyErrorKind::Parse(msg))
    }

    /// Constructor for a generic runtime error.
    #[cold]
    #[inline]
    pub fn runtime(msg: String) -> Self {
        Self::new(PyErrorKind::Runtime(msg))
    }

    /// Convenience constructor for a named Python exception with a static
    /// class-name literal.  Avoids the per-call `"TypeError".to_string()`
    /// allocation that every error site would otherwise perform.
    ///
    /// Prefer [`PyError::class`] when the class `Rc` is already in scope
    /// (avoids an env-hierarchy name lookup in the VM handler).
    #[cold]
    #[inline]
    pub fn named(cls: &'static str, msg: impl Into<String>) -> Self {
        Self::new(PyErrorKind::Named(Cow::Borrowed(cls), msg.into()))
    }

    /// Constructor for a named Python exception with a **known class object**.
    ///
    /// The VM can materialise this directly (no env name lookup).  Use this
    /// from interpreter-internal raise sites that already hold the class `Rc`.
    #[cold]
    #[inline]
    pub fn class(cls: Rc<RefCell<PyClass>>, msg: impl Into<String>) -> Self {
        Self::new(PyErrorKind::Class(cls, msg.into()))
    }

    /// Constructor for a `KeyError` that stores the raw key `Value`.
    ///
    /// CPython keeps the original key object as `args[0]` of the `KeyError`
    /// instance, so `e.args[0]` returns the key itself (not a repr string).
    /// Use this at every dict/set key-not-found raise site instead of
    /// `PyError::named("KeyError", key.repr_raw())`.
    #[cold]
    #[inline]
    pub fn key_error(key: Value) -> Self {
        Self::new(PyErrorKind::KeyError(key))
    }

    /// Constructor for an `ImportError` or `ModuleNotFoundError` that carries
    /// the module name.
    ///
    /// `class_name` must be `"ImportError"` or `"ModuleNotFoundError"`.
    /// The VM materialises the exception and sets `.name` and `.path` on it.
    #[cold]
    #[inline]
    pub fn import_error(
        class_name: &'static str,
        message: impl Into<String>,
        module_name: Option<String>,
    ) -> Self {
        Self::new(PyErrorKind::ImportError {
            class_name,
            message: message.into(),
            module_name,
        })
    }

    /// Constructor for a `NameError` or `UnboundLocalError` that carries the
    /// identifier name so the VM can set `.name` on the resulting instance.
    ///
    /// `class_name` must be `"NameError"` or `"UnboundLocalError"`.
    /// `name` is the identifier that was not found; pass `None` for
    /// `UnboundLocalError` (CPython 3.12: `UnboundLocalError.name` is `None`).
    #[cold]
    #[inline]
    pub fn name_error(
        class_name: &'static str,
        message: impl Into<String>,
        name: Option<String>,
    ) -> Self {
        Self::new(PyErrorKind::NameError {
            class_name,
            message: message.into(),
            name,
        })
    }

    /// Constructor for an `AttributeError` that carries the attribute name and
    /// the receiver object so the VM can set `.name` and `.obj` on the
    /// resulting instance, matching CPython 3.12 parity.
    ///
    /// `name` is the attribute that was not found; pass `None` when not
    /// available.  `obj` is the receiver; pass `None` when not available.
    #[cold]
    #[inline]
    pub fn attribute_error(
        message: impl Into<String>,
        name: Option<String>,
        obj: Option<Value>,
    ) -> Self {
        Self::new(PyErrorKind::AttributeError {
            message: message.into(),
            name,
            obj,
        })
    }

    /// Constructor for a located compile-time syntax error (issue #3402).
    #[cold]
    pub fn syntax(info: SyntaxErrorInfo) -> Self {
        Self::new(PyErrorKind::Syntax(Box::new(info)))
    }

    /// The message of a lexer / parser / located syntax error, or `None` for
    /// every other error kind.
    pub fn syntax_message(&self) -> Option<&str> {
        match self.kind() {
            PyErrorKind::Lex(msg) | PyErrorKind::Parse(msg) => Some(msg),
            PyErrorKind::Syntax(info) => Some(&info.msg),
            _ => None,
        }
    }

    /// Constructor for an already-materialised Python exception value.
    #[cold]
    #[inline]
    pub fn raised(value: Value) -> Self {
        Self::new(PyErrorKind::Raised(value))
    }

    /// Map a `std::io::ErrorKind` to the most-derived Python OSError subclass
    /// name following CPython 3.12's errno-to-subclass mapping.
    fn io_kind_to_class(kind: std::io::ErrorKind) -> &'static str {
        use std::io::ErrorKind::*;
        match kind {
            NotFound => "FileNotFoundError",
            PermissionDenied => "PermissionError",
            AlreadyExists => "FileExistsError",
            IsADirectory => "IsADirectoryError",
            NotADirectory => "NotADirectoryError",
            Interrupted => "InterruptedError",
            WouldBlock => "BlockingIOError",
            TimedOut => "TimeoutError",
            _ => "OSError",
        }
    }

    /// Convert a `std::io::Error` into a Python `OSError` (or subclass),
    /// attaching `filename` when provided.
    ///
    /// The `strerror` stored on the exception is the OS-provided message
    /// stripped of any file-path decoration, matching CPython's behaviour
    /// where `e.strerror` is `"No such file or directory"` rather than the
    /// decorated form.
    #[cold]
    pub fn from_io_error(e: &std::io::Error, filename: Option<&str>) -> Self {
        Self::from_io_error2(e, filename, None)
    }

    /// Like [`from_io_error`] but also sets `filename2` for two-path operations
    /// (e.g. `os.rename(src, dst)` where `filename2` is the destination).
    /// CPython 3.12 sets `filename2` on the exception for rename/link/symlink.
    #[cold]
    pub fn from_io_error2(
        e: &std::io::Error,
        filename: Option<&str>,
        filename2: Option<&str>,
    ) -> Self {
        let raw = e.raw_os_error().unwrap_or(0);
        let class_name = Self::io_kind_to_class(e.kind());
        // `std::io::Error::from_raw_os_error(N).to_string()` on Linux produces
        // "strerror(N) (os error N)" — strip the Rust-added trailer.
        // On Windows, FormatMessage strings end with a trailing period; strip
        // that too so the result matches CPython's `e.strerror` behaviour.
        let strerror = if raw != 0 {
            let full = std::io::Error::from_raw_os_error(raw).to_string();
            let base = if let Some(pos) = full.rfind(" (os error ") {
                &full[..pos]
            } else {
                &full[..]
            };
            base.trim_end_matches('.').to_owned()
        } else {
            e.to_string()
        };
        Self::new(PyErrorKind::OsError {
            class_name,
            errno: raw as i64,
            strerror,
            filename: filename.map(|s| s.to_owned()),
            filename2: filename2.map(|s| s.to_owned()),
        })
    }

    /// Returns `true` when `self` is a `Named`, `Class`, `KeyError`, or `Raised` error
    /// whose exception class name equals `name`.
    ///
    /// Used by the generator/iterator machinery to cheaply detect
    /// `StopIteration` and `GeneratorExit` without materialising the error
    /// into a full `PyInstance`.  Works for the string-named (`Named`),
    /// class-identity (`Class`), and already-raised-instance (`Raised`)
    /// variants so that `PyErrorKind::Raised(StopIteration(...))` produced by
    /// `resume_generator` is treated the same as `PyErrorKind::Named`.
    #[inline]
    pub fn class_name_is(&self, name: &str) -> bool {
        match self.kind() {
            PyErrorKind::Named(cls, _) => cls.as_ref() == name,
            PyErrorKind::Class(cls, _) => class_chain_contains_builtin_exception(cls, name),
            PyErrorKind::KeyError(_) => name == "KeyError",
            PyErrorKind::ImportError { class_name, .. } => {
                // ModuleNotFoundError is a subclass of ImportError; treat it
                // as both so that `class_name_is("ImportError")` returns true
                // for a ModuleNotFoundError variant too.
                *class_name == name
                    || (name == "ImportError" && *class_name == "ModuleNotFoundError")
            }
            PyErrorKind::OsError { class_name, .. } => {
                // All OSError subclasses also match "OSError" for the fast-path check.
                *class_name == name
                    || (name == "OSError"
                        && matches!(
                            *class_name,
                            "FileNotFoundError"
                                | "PermissionError"
                                | "FileExistsError"
                                | "IsADirectoryError"
                                | "NotADirectoryError"
                                | "InterruptedError"
                                | "BlockingIOError"
                                | "ChildProcessError"
                                | "ProcessLookupError"
                                | "TimeoutError"
                        ))
            }
            PyErrorKind::UnicodeDecodeError { .. } => {
                name == "UnicodeDecodeError"
                    || name == "UnicodeError"
                    || name == "ValueError"
                    || name == "Exception"
                    || name == "BaseException"
            }
            PyErrorKind::UnicodeEncodeError { .. } => {
                name == "UnicodeEncodeError"
                    || name == "UnicodeError"
                    || name == "ValueError"
                    || name == "Exception"
                    || name == "BaseException"
            }
            PyErrorKind::AttributeError { .. } => {
                // AttributeError is a subclass of Exception → BaseException.
                matches!(name, "AttributeError" | "Exception" | "BaseException")
            }
            PyErrorKind::Syntax(info) => {
                // TabError < IndentationError < SyntaxError < Exception.
                name == info.class_name
                    || matches!(name, "SyntaxError" | "Exception" | "BaseException")
                    || (name == "IndentationError" && info.class_name == "TabError")
            }
            PyErrorKind::Raised(exc) => match exc.kind() {
                ValueKind::PyInstance(inst) => {
                    class_chain_contains_builtin_exception(&inst.borrow().class, name)
                }
                ValueKind::PyClass(cls) => class_chain_contains_builtin_exception(cls, name),
                _ => false,
            },
            _ => false,
        }
    }
}

impl fmt::Display for PyErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PyErrorKind::Lex(s) => write!(f, "Lex error: {s}"),
            PyErrorKind::Parse(s) => write!(f, "Parse error: {s}"),
            PyErrorKind::Runtime(s) => write!(f, "Runtime error: {s}"),
            PyErrorKind::Named(cls, s) => write!(f, "{cls}: {s}"),
            PyErrorKind::Class(cls, s) => write!(f, "{}: {s}", cls.borrow().name),
            PyErrorKind::KeyError(key) => write!(f, "KeyError: {}", key.repr_raw()),
            PyErrorKind::ImportError {
                class_name,
                message,
                ..
            } => {
                write!(f, "{class_name}: {message}")
            }
            PyErrorKind::OsError {
                class_name,
                errno,
                strerror,
                filename,
                ..
            } => {
                if let Some(fname) = filename {
                    write!(f, "{class_name}: [Errno {errno}] {strerror}: '{fname}'")
                } else {
                    write!(f, "{class_name}: [Errno {errno}] {strerror}")
                }
            }
            PyErrorKind::UnicodeDecodeError {
                encoding,
                object,
                start,
                end,
                reason,
            } => {
                let msg = format_unicode_decode_str(encoding, object, *start, *end, reason);
                write!(f, "UnicodeDecodeError: {msg}")
            }
            PyErrorKind::UnicodeEncodeError {
                encoding,
                object,
                start,
                end,
                reason,
            } => {
                let msg = format_unicode_encode_str(encoding, object, *start, *end, reason);
                write!(f, "UnicodeEncodeError: {msg}")
            }
            PyErrorKind::NameError {
                class_name,
                message,
                ..
            } => {
                write!(f, "{class_name}: {message}")
            }
            PyErrorKind::AttributeError { message, .. } => {
                write!(f, "AttributeError: {message}")
            }
            PyErrorKind::Syntax(info) => write!(f, "{}: {}", info.class_name, info.msg),
            PyErrorKind::Raised(value) => write!(f, "Uncaught exception: {}", value.repr_raw()),
        }
    }
}

impl fmt::Display for PyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.kind(), f)
    }
}

pub type Result<T> = std::result::Result<T, PyError>;

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::{PyError, PyErrorKind, Result};
    use crate::object_model::Value;

    #[test]
    fn unboxed_error_box_is_reused_and_retains_no_value() {
        // A private thread owns the spare box this test observes.
        std::thread::spawn(|| {
            let payload = Value::list(vec![Value::int(1)]);
            let error = PyError::raised(payload.clone());
            let first_box = std::ptr::from_ref(error.kind());

            let kind = error.into_kind();
            assert!(matches!(&kind, PyErrorKind::Raised(value) if value.is_identical_to(&payload)));
            drop(kind);
            assert_eq!(
                payload.refcount(),
                1,
                "the parked box must not keep the raised value alive"
            );

            // The next error reuses the parked box and carries its own payload.
            let next = PyError::runtime("next".to_string());
            assert_eq!(std::ptr::from_ref(next.kind()), first_box);
            assert!(matches!(next.kind(), PyErrorKind::Runtime(message) if message == "next"));

            // With the spare in use, a second live error gets its own box.
            let other = PyError::runtime("other".to_string());
            assert_ne!(std::ptr::from_ref(other.kind()), first_box);
            assert!(
                matches!(other.into_kind(), PyErrorKind::Runtime(message) if message == "other")
            );
            assert!(matches!(next.into_kind(), PyErrorKind::Runtime(message) if message == "next"));
        })
        .join()
        .expect("error box reuse must not panic");
    }

    #[test]
    fn pyerror_and_result_sizes_stay_compact() {
        assert_eq!(
            size_of::<PyError>(),
            8,
            "PyError must stay a single boxed pointer (8 B)"
        );
        assert_eq!(
            size_of::<Result<Value>>(),
            16,
            "Result<Value, PyError> must stay 16 B (two registers)"
        );
        assert_eq!(
            size_of::<Result<()>>(),
            8,
            "Result<(), PyError> must use Box non-null niche (8 B)"
        );
        assert_eq!(
            size_of::<Option<PyError>>(),
            8,
            "Option<PyError> must use Box non-null niche (8 B)"
        );
    }
}
