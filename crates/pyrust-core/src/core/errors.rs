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

#[derive(Debug, Clone)]
pub enum PyError {
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
    /// continue to use [`PyError::named`] / `PyError::Named`.
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
    /// of `PyError::Named("ImportError", …)` so that `e.name` works in
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
    Raised(Value),
}

impl PyError {
    /// Convenience constructor for a named Python exception with a static
    /// class-name literal.  Avoids the per-call `"TypeError".to_string()`
    /// allocation that every error site would otherwise perform.
    ///
    /// Prefer [`PyError::class`] when the class `Rc` is already in scope
    /// (avoids an env-hierarchy name lookup in the VM handler).
    #[inline]
    pub fn named(cls: &'static str, msg: impl Into<String>) -> Self {
        PyError::Named(Cow::Borrowed(cls), msg.into())
    }

    /// Constructor for a named Python exception with a **known class object**.
    ///
    /// The VM can materialise this directly (no env name lookup).  Use this
    /// from interpreter-internal raise sites that already hold the class `Rc`.
    #[inline]
    pub fn class(cls: Rc<RefCell<PyClass>>, msg: impl Into<String>) -> Self {
        PyError::Class(cls, msg.into())
    }

    /// Constructor for a `KeyError` that stores the raw key `Value`.
    ///
    /// CPython keeps the original key object as `args[0]` of the `KeyError`
    /// instance, so `e.args[0]` returns the key itself (not a repr string).
    /// Use this at every dict/set key-not-found raise site instead of
    /// `PyError::named("KeyError", key.repr_raw())`.
    #[inline]
    pub fn key_error(key: Value) -> Self {
        PyError::KeyError(key)
    }

    /// Constructor for an `ImportError` or `ModuleNotFoundError` that carries
    /// the module name.
    ///
    /// `class_name` must be `"ImportError"` or `"ModuleNotFoundError"`.
    /// The VM materialises the exception and sets `.name` and `.path` on it.
    #[inline]
    pub fn import_error(
        class_name: &'static str,
        message: impl Into<String>,
        module_name: Option<String>,
    ) -> Self {
        PyError::ImportError {
            class_name,
            message: message.into(),
            module_name,
        }
    }

    /// Constructor for a `NameError` or `UnboundLocalError` that carries the
    /// identifier name so the VM can set `.name` on the resulting instance.
    ///
    /// `class_name` must be `"NameError"` or `"UnboundLocalError"`.
    /// `name` is the identifier that was not found; pass `None` for
    /// `UnboundLocalError` (CPython 3.12: `UnboundLocalError.name` is `None`).
    #[inline]
    pub fn name_error(
        class_name: &'static str,
        message: impl Into<String>,
        name: Option<String>,
    ) -> Self {
        PyError::NameError {
            class_name,
            message: message.into(),
            name,
        }
    }

    /// Constructor for an `AttributeError` that carries the attribute name and
    /// the receiver object so the VM can set `.name` and `.obj` on the
    /// resulting instance, matching CPython 3.12 parity.
    ///
    /// `name` is the attribute that was not found; pass `None` when not
    /// available.  `obj` is the receiver; pass `None` when not available.
    #[inline]
    pub fn attribute_error(
        message: impl Into<String>,
        name: Option<String>,
        obj: Option<Value>,
    ) -> Self {
        PyError::AttributeError {
            message: message.into(),
            name,
            obj,
        }
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
    pub fn from_io_error(e: &std::io::Error, filename: Option<&str>) -> Self {
        Self::from_io_error2(e, filename, None)
    }

    /// Like [`from_io_error`] but also sets `filename2` for two-path operations
    /// (e.g. `os.rename(src, dst)` where `filename2` is the destination).
    /// CPython 3.12 sets `filename2` on the exception for rename/link/symlink.
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
        PyError::OsError {
            class_name,
            errno: raw as i64,
            strerror,
            filename: filename.map(|s| s.to_owned()),
            filename2: filename2.map(|s| s.to_owned()),
        }
    }

    /// Returns `true` when `self` is a `Named`, `Class`, `KeyError`, or `Raised` error
    /// whose exception class name equals `name`.
    ///
    /// Used by the generator/iterator machinery to cheaply detect
    /// `StopIteration` and `GeneratorExit` without materialising the error
    /// into a full `PyInstance`.  Works for the string-named (`Named`),
    /// class-identity (`Class`), and already-raised-instance (`Raised`)
    /// variants so that `PyError::Raised(StopIteration(...))` produced by
    /// `resume_generator` is treated the same as `PyError::Named`.
    #[inline]
    pub fn class_name_is(&self, name: &str) -> bool {
        match self {
            PyError::Named(cls, _) => cls.as_ref() == name,
            PyError::Class(cls, _) => class_chain_contains_builtin_exception(cls, name),
            PyError::KeyError(_) => name == "KeyError",
            PyError::ImportError { class_name, .. } => {
                // ModuleNotFoundError is a subclass of ImportError; treat it
                // as both so that `class_name_is("ImportError")` returns true
                // for a ModuleNotFoundError variant too.
                *class_name == name
                    || (name == "ImportError" && *class_name == "ModuleNotFoundError")
            }
            PyError::OsError { class_name, .. } => {
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
            PyError::UnicodeDecodeError { .. } => {
                name == "UnicodeDecodeError"
                    || name == "UnicodeError"
                    || name == "ValueError"
                    || name == "Exception"
                    || name == "BaseException"
            }
            PyError::UnicodeEncodeError { .. } => {
                name == "UnicodeEncodeError"
                    || name == "UnicodeError"
                    || name == "ValueError"
                    || name == "Exception"
                    || name == "BaseException"
            }
            PyError::AttributeError { .. } => {
                // AttributeError is a subclass of Exception → BaseException.
                matches!(name, "AttributeError" | "Exception" | "BaseException")
            }
            PyError::Raised(exc) => match exc.kind() {
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

impl fmt::Display for PyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PyError::Lex(s) => write!(f, "Lex error: {s}"),
            PyError::Parse(s) => write!(f, "Parse error: {s}"),
            PyError::Runtime(s) => write!(f, "Runtime error: {s}"),
            PyError::Named(cls, s) => write!(f, "{cls}: {s}"),
            PyError::Class(cls, s) => write!(f, "{}: {s}", cls.borrow().name),
            PyError::KeyError(key) => write!(f, "KeyError: {}", key.repr_raw()),
            PyError::ImportError {
                class_name,
                message,
                ..
            } => {
                write!(f, "{class_name}: {message}")
            }
            PyError::OsError {
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
            PyError::UnicodeDecodeError {
                encoding,
                object,
                start,
                end,
                reason,
            } => {
                let msg = format_unicode_decode_str(encoding, object, *start, *end, reason);
                write!(f, "UnicodeDecodeError: {msg}")
            }
            PyError::UnicodeEncodeError {
                encoding,
                object,
                start,
                end,
                reason,
            } => {
                let msg = format_unicode_encode_str(encoding, object, *start, *end, reason);
                write!(f, "UnicodeEncodeError: {msg}")
            }
            PyError::NameError {
                class_name,
                message,
                ..
            } => {
                write!(f, "{class_name}: {message}")
            }
            PyError::AttributeError { message, .. } => {
                write!(f, "AttributeError: {message}")
            }
            PyError::Raised(value) => write!(f, "Uncaught exception: {}", value.repr_raw()),
        }
    }
}

pub type Result<T> = std::result::Result<T, PyError>;
