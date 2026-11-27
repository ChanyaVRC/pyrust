//! Per-site inline cache for the fused zero-argument `super().name(...)` call
//! (#3424).
//!
//! `Insn::LoadSuperMethod` resolves `name` along the MRO of `type(self)`,
//! starting after the method's defining class (`__class__`). That answer is a
//! pure function of three things: the receiver's class, the defining class,
//! and the contents of every class on that MRO. An entry therefore records the
//! two class identities plus the same `(mutation_version, class epoch)` stamp
//! the `GetAttr` / `CallMethod` caches use, and maps them to the resolved
//! method — the MRO position is implied, so a hit performs no MRO walk at all.
//!
//! A site is polymorphic in the receiver class by nature: a method that calls
//! `super().name()` runs for instances of every subclass, each with its own
//! MRO. A site keeps a few entries before giving up on caching.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use pyrust_core::{PyClass, WeakValueCache};

use crate::value::Value;

/// Distinct `(receiver class, defining class)` pairs one site remembers.
pub(crate) const SUPER_METHOD_CACHE_WAYS: usize = 8;

/// One resolved `(receiver class, defining class)` pair.
///
/// Both classes are held weakly: the code object must not keep a class alive,
/// and a retained weak allocation cannot be reused by a different class (the
/// ABA hazard a raw pointer would have).
#[derive(Clone, Debug)]
pub(crate) struct SuperMethodCacheEntry {
    /// `type(self)` observed when the entry was filled.
    self_class: Weak<RefCell<PyClass>>,
    /// The `__class__` cell observed when the entry was filled.
    start_class: Weak<RefCell<PyClass>>,
    /// `PyClass::mutation_version` of `self_class` at fill time.
    class_version: u64,
    /// `pyrust_core::class_epoch()` at fill time (catches edits anywhere on
    /// the MRO, including a re-parented `__bases__`).
    epoch: u64,
    /// The plain Python function the lookup resolved to, or `None` when it
    /// resolved to anything else (a descriptor, a builtin, a missing name), in
    /// which case the site takes the generic path without repeating the walk.
    ///
    /// Weak for the same reason as the method caches: the function can own
    /// the very code object that holds this cache.
    target: Option<WeakValueCache>,
}

impl SuperMethodCacheEntry {
    #[inline]
    fn is_for(
        &self,
        self_class: &Rc<RefCell<PyClass>>,
        start_class: &Rc<RefCell<PyClass>>,
    ) -> bool {
        std::ptr::eq(self.self_class.as_ptr(), Rc::as_ptr(self_class))
            && std::ptr::eq(self.start_class.as_ptr(), Rc::as_ptr(start_class))
    }
}

/// Outcome of probing a site for one `(receiver class, defining class)` pair.
pub(crate) enum SuperMethodProbe {
    /// The pair resolves to this plain Python function.
    Method(Value),
    /// The pair is known not to resolve to a plain Python function.
    NotPlainMethod,
    /// Nothing usable is cached for the pair; resolve it and call
    /// [`SuperMethodSite::record`].
    Miss,
}

/// Per-`LoadSuperMethod`-site cache state.
#[derive(Clone, Debug, Default)]
pub(crate) enum SuperMethodSite {
    /// Nothing observed yet.
    #[default]
    Empty,
    /// Up to [`SUPER_METHOD_CACHE_WAYS`] resolved pairs.
    Entries(Vec<SuperMethodCacheEntry>),
    /// More pairs than the site can hold: resolve on every execution.
    Megamorphic,
    /// The first resolution was not a plain method (`super().__init__()` into
    /// a builtin base, a property, ...): the site always takes the generic
    /// path and never pays for a resolution attempt again.
    Generic,
}

impl SuperMethodSite {
    /// Whether the site wants a resolution attempt at all.
    #[inline]
    pub(crate) fn is_generic(&self) -> bool {
        matches!(self, Self::Generic)
    }

    /// `(plain-method pairs, other pairs)` currently cached, for tests.
    #[cfg(test)]
    pub(crate) fn cached_pairs(&self) -> Option<(usize, usize)> {
        let Self::Entries(entries) = self else {
            return None;
        };
        let methods = entries
            .iter()
            .filter(|entry| entry.target.is_some())
            .count();
        Some((methods, entries.len() - methods))
    }

    /// Look up `(self_class, start_class)`; `class_version` is the current
    /// `mutation_version` of `self_class`.
    #[inline]
    pub(crate) fn probe(
        &self,
        self_class: &Rc<RefCell<PyClass>>,
        start_class: &Rc<RefCell<PyClass>>,
        class_version: u64,
    ) -> SuperMethodProbe {
        let Self::Entries(entries) = self else {
            return SuperMethodProbe::Miss;
        };
        for entry in entries {
            if !entry.is_for(self_class, start_class) {
                continue;
            }
            if !pyrust_core::class_cache_stamp_matches(
                class_version,
                entry.class_version,
                entry.epoch,
            ) {
                return SuperMethodProbe::Miss;
            }
            return match &entry.target {
                // A dead function means the class attribute was replaced by a
                // path that bypassed the stamp; resolve again.
                Some(target) => target
                    .upgrade()
                    .map_or(SuperMethodProbe::Miss, SuperMethodProbe::Method),
                None => SuperMethodProbe::NotPlainMethod,
            };
        }
        SuperMethodProbe::Miss
    }

    /// Record a fresh resolution for `(self_class, start_class)`: `target` is
    /// the plain Python function it found, or `None` for any other outcome.
    pub(crate) fn record(
        &mut self,
        self_class: &Rc<RefCell<PyClass>>,
        start_class: &Rc<RefCell<PyClass>>,
        class_version: u64,
        target: Option<&Value>,
    ) {
        if matches!(self, Self::Megamorphic | Self::Generic) {
            return;
        }
        let Some((class_version, epoch)) = pyrust_core::class_cache_stamp(class_version) else {
            return;
        };
        let target = match target {
            Some(value) => match WeakValueCache::new(value) {
                Some(weak) => Some(weak),
                // Not representable without owning the function: leave the
                // pair uncached so it is simply resolved each time.
                None => return,
            },
            None => None,
        };
        let entry = SuperMethodCacheEntry {
            self_class: Rc::downgrade(self_class),
            start_class: Rc::downgrade(start_class),
            class_version,
            epoch,
            target,
        };
        match self {
            Self::Empty => {
                *self = if entry.target.is_some() {
                    Self::Entries(vec![entry])
                } else {
                    Self::Generic
                };
            }
            Self::Entries(entries) => {
                if let Some(stale) = entries
                    .iter_mut()
                    .find(|cached| cached.is_for(self_class, start_class))
                {
                    *stale = entry;
                    return;
                }
                if entries.len() >= SUPER_METHOD_CACHE_WAYS {
                    // Classes built in a loop leave dead pairs behind.
                    entries.retain(|cached| {
                        cached.self_class.strong_count() > 0
                            && cached.start_class.strong_count() > 0
                    });
                }
                if entries.len() < SUPER_METHOD_CACHE_WAYS {
                    entries.push(entry);
                } else {
                    *self = Self::Megamorphic;
                }
            }
            Self::Megamorphic | Self::Generic => {}
        }
    }
}
