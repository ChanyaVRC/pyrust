//! Per-site `CallMethod` cache for built-in receivers (#3379).
//!
//! The generic [`super::AttrCacheEntry`] protocol caches the *unbound value* a
//! `CallMethod` site resolved on a user/native class and re-binds it on every
//! hit.  This cache instead records the *typed dispatch target* a site
//! resolved for a built-in receiver, so a warm hit skips receiver
//! classification by method name, registry-key resolution, and receiver-value
//! materialisation:
//!
//! - [`BuiltinMethodCacheEntry::Container`]: an exact built-in container
//!   receiver (`list`, `dict`, `set`, `tuple`, `str`, `bytes`).  Guarded by the
//!   receiver's container kind; subclass instances are never exact containers.
//! - [`BuiltinMethodCacheEntry::NativeMethod`]: an instance of a native class
//!   (`collections.deque`, `Counter`, …) whose method resolved to a registry
//!   method descriptor.  Guarded exactly like `AttrCacheEntry::ClassAttr`:
//!   weak class identity, `PyClass::mutation_version`, the global class epoch,
//!   and the absence of an instance-dict shadow.
//!
//! The typed targets themselves are opaque handles owned by
//! `builtin_methods`; this module stores them and never interprets method
//! names.

use crate::bytecode::Insn;

/// Returns `true` if `insn` can populate
/// [`crate::bytecode::FnCode::builtin_method_cache`].
#[inline]
pub(crate) fn is_builtin_method_cache_site(insn: &Insn) -> bool {
    matches!(insn, Insn::CallMethod { .. })
}

/// Boxed payload for [`BuiltinMethodCacheEntry::NativeMethod`] so the enum
/// stays small for the far more common container entries.
#[derive(Clone, Debug)]
pub(crate) struct NativeMethodCacheEntry {
    /// Weak allocation identity of the receiver class observed at fill time
    /// (weak so the cache neither extends the class lifetime nor suffers ABA
    /// address reuse).
    pub(crate) class_ptr: std::rc::Weak<std::cell::RefCell<pyrust_core::PyClass>>,
    /// `PyClass::mutation_version` at fill time.
    pub(crate) class_version: u64,
    /// `pyrust_core::class_epoch()` at fill time (catches MRO-ancestor edits).
    pub(crate) epoch: u64,
    /// Typed dispatch target resolved by `builtin_methods`.
    pub(crate) method: crate::interpreter::DirectNativeMethod,
}

/// Per-`CallMethod`-site state for built-in receiver dispatch.
#[derive(Clone, Debug)]
pub(crate) enum BuiltinMethodCacheEntry {
    /// Not yet observed (or invalidated for a refill).
    Empty,
    /// Exact built-in container receiver resolved to a typed method route.
    Container(crate::interpreter::DirectContainerMethod),
    /// Native-class instance method resolved to its registry dispatch entry.
    NativeMethod(Box<NativeMethodCacheEntry>),
    /// The site saw several receiver shapes or an uncacheable resolution;
    /// it permanently takes the generic path.
    Megamorphic,
}
