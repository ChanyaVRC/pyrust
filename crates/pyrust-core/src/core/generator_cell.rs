// Storage behind `ValueKind::Generator`: the object's immutable Python type
// tag and its Python-visible display names, kept *outside* the type-erased
// execution state so that classification never has to borrow a frame that is
// currently running (issue #2978).

/// Which Python type a generator-tagged value presents as.
///
/// Fixed when the object is created and never mutated, so `type()`,
/// `isinstance`, `repr()`, `dir()` and the copy-refusal noun can all be
/// answered while the object's body is executing — the point at which its
/// state cell is mutably checked out and unreadable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GeneratorKind {
    /// A built-in iterator (`map`, `zip`, `range_iterator`, a provider
    /// iterator, …).  These release the state cell before running any user
    /// code, so their finer-grained type name is still read from the concrete
    /// state itself rather than from this tag.
    Iterator,
    /// A `def` body containing `yield`.
    Generator,
    /// An `async def` body without `yield`.
    Coroutine,
    /// An `async def` body containing `yield`.
    AsyncGenerator,
}

impl GeneratorKind {
    /// The CPython type name for a Python frame object.
    ///
    /// `None` for [`GeneratorKind::Iterator`], which covers many concrete
    /// built-in iterator types that share no single name.
    pub fn frame_type_name(self) -> Option<&'static str> {
        match self {
            GeneratorKind::Iterator => None,
            GeneratorKind::Generator => Some("generator"),
            GeneratorKind::Coroutine => Some("coroutine"),
            GeneratorKind::AsyncGenerator => Some("async_generator"),
        }
    }
}

/// The writable `__name__` / `__qualname__` pair of a generator frame object.
///
/// Separate from the frame's compile-time `fn_name` / `qualname` (which back
/// tracebacks and `co_name` / `co_qualname`, and which CPython does not let a
/// `g.__name__ = …` assignment change).
pub struct GeneratorNames {
    pub name: std::sync::Arc<str>,
    pub qualname: std::sync::Arc<str>,
}

/// The allocation a `ValueKind::Generator` value points at.
///
/// [`Deref`](std::ops::Deref)s to the state cell so the existing
/// `state_rc.borrow()` / `try_borrow_mut()` call sites keep working unchanged;
/// the tag and the names are reached through the accessors and are readable
/// even while that cell is checked out.
pub struct GeneratorCell {
    kind: GeneratorKind,
    /// `None` for built-in iterators, which expose no `__name__`.
    names: RefCell<Option<GeneratorNames>>,
    state: RefCell<Box<dyn Any>>,
}

impl GeneratorCell {
    /// The immutable Python type tag.
    pub fn kind(&self) -> GeneratorKind {
        self.kind
    }

    /// The Python-visible `__name__`, or `None` for a built-in iterator.
    pub fn name(&self) -> Option<std::sync::Arc<str>> {
        self.names.borrow().as_ref().map(|n| n.name.clone())
    }

    /// The Python-visible `__qualname__`, or `None` for a built-in iterator.
    pub fn qualname(&self) -> Option<std::sync::Arc<str>> {
        self.names.borrow().as_ref().map(|n| n.qualname.clone())
    }

    /// Overwrite `__name__`.  No-op on a built-in iterator, which has none.
    pub fn set_name(&self, value: &str) {
        if let Some(names) = self.names.borrow_mut().as_mut() {
            names.name = std::sync::Arc::from(value);
        }
    }

    /// Overwrite `__qualname__`.  No-op on a built-in iterator.
    pub fn set_qualname(&self, value: &str) {
        if let Some(names) = self.names.borrow_mut().as_mut() {
            names.qualname = std::sync::Arc::from(value);
        }
    }
}

impl std::ops::Deref for GeneratorCell {
    type Target = RefCell<Box<dyn Any>>;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
