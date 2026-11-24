//! Compact lazily-allocated storage backing per-PC inline caches.
//!
//! Two strategies match the two density regimes of `FnCode` inline caches:
//! - [`SiteCache`]: O(1) array-indexed lookup for hot, moderately dense caches
//!   (`attr_cache`, `call_builtin_cache`). Starts empty (0 heap bytes); on
//!   first cache write, scans `insns` once to pack only matching instruction
//!   sites into a compact `pc -> slot` table (`u8` when `<= 255` sites) paired
//!   with a dense `Box<[T]>` of entries.
//! - [`SparsePcCache`]: sorted `Vec<(u32, T)>` for ultra-sparse caches
//!   (`kwcall_cache`, `fmt_spec_cache`). Starts empty (0 heap bytes) and
//!   allocates storage strictly proportional to the number of populated sites.

use crate::bytecode::Insn;

/// Returns `true` if `insn` can populate [`crate::bytecode::FnCode::attr_cache`].
#[inline]
pub(crate) fn is_attr_cache_site(insn: &Insn) -> bool {
    matches!(
        insn,
        Insn::GetAttr(..) | Insn::SetAttr(..) | Insn::CallMethod { .. } | Insn::CallMethodKw(..)
    )
}

/// Returns `true` if `insn` can populate [`crate::bytecode::FnCode::call_builtin_cache`].
#[inline]
pub(crate) fn is_call_builtin_cache_site(insn: &Insn) -> bool {
    matches!(insn, Insn::Call(..) | Insn::CallMemo(..))
}

#[derive(Debug, Clone)]
enum SiteCacheStorage<T> {
    /// Uninitialized — 0 bytes allocated on the heap.
    Empty,
    /// Compact site-indexed layout for functions with `<= 255` matching sites.
    /// `u8::MAX` marks non-matching PCs; valid slots are `0..entries.len()`
    /// where `entries.len() <= u8::MAX as usize`, so `entries.get(u8::MAX as usize)`
    /// naturally returns `None`.
    Small {
        pc_to_slot: Box<[u8]>,
        entries: Box<[T]>,
    },
    /// Compact site-indexed layout for functions with `256..=65535` matching sites.
    /// `u16::MAX` marks non-matching PCs; valid slots are `0..entries.len()`
    /// where `entries.len() <= u16::MAX as usize`.
    Medium {
        pc_to_slot: Box<[u16]>,
        entries: Box<[T]>,
    },
    /// Fallback dense layout for extraordinarily large functions with `> 65535`
    /// matching sites.
    Dense(Box<[T]>),
}

/// Compact, lazily-allocated, O(1)-indexed per-PC inline cache.
#[derive(Debug, Clone)]
pub(crate) struct SiteCache<T> {
    storage: SiteCacheStorage<T>,
}

impl<T> SiteCache<T> {
    /// Create an empty cache with 0 bytes of heap allocation.
    #[inline]
    pub(crate) const fn new() -> Self {
        Self {
            storage: SiteCacheStorage::Empty,
        }
    }

    /// True when no heap storage has been allocated yet.
    #[cfg(test)]
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        matches!(self.storage, SiteCacheStorage::Empty)
    }

    /// O(1) array-indexed lookup by instruction PC.
    ///
    /// Returns `None` when the cache has not been initialized yet or when `pc`
    /// is not a matching site.
    #[inline(always)]
    pub(crate) fn get(&self, pc: usize) -> Option<&T> {
        match &self.storage {
            SiteCacheStorage::Empty => None,
            SiteCacheStorage::Small {
                pc_to_slot,
                entries,
            } => {
                let slot = *pc_to_slot.get(pc)? as usize;
                entries.get(slot)
            }
            SiteCacheStorage::Medium {
                pc_to_slot,
                entries,
            } => {
                let slot = *pc_to_slot.get(pc)? as usize;
                entries.get(slot)
            }
            SiteCacheStorage::Dense(entries) => entries.get(pc),
        }
    }

    /// Return a mutable reference to `pc`'s slot, lazily initializing the
    /// compact site table from `insns` on first write.
    #[inline]
    pub(crate) fn get_mut_or_init(
        &mut self,
        insns: &[Insn],
        pc: usize,
        is_site: impl Fn(&Insn) -> bool,
        empty: impl Fn() -> T,
    ) -> Option<&mut T> {
        if matches!(self.storage, SiteCacheStorage::Empty) {
            self.init(insns, pc, is_site, empty);
        }
        match &mut self.storage {
            SiteCacheStorage::Empty => None,
            SiteCacheStorage::Small {
                pc_to_slot,
                entries,
            } => {
                let slot = *pc_to_slot.get(pc)? as usize;
                entries.get_mut(slot)
            }
            SiteCacheStorage::Medium {
                pc_to_slot,
                entries,
            } => {
                let slot = *pc_to_slot.get(pc)? as usize;
                entries.get_mut(slot)
            }
            SiteCacheStorage::Dense(entries) => entries.get_mut(pc),
        }
    }

    /// Write `value` into `pc`'s slot, lazily initializing the compact site
    /// table on first write.
    #[inline]
    pub(crate) fn set_or_init(
        &mut self,
        insns: &[Insn],
        pc: usize,
        is_site: impl Fn(&Insn) -> bool,
        empty: impl Fn() -> T,
        value: T,
    ) {
        if let Some(slot) = self.get_mut_or_init(insns, pc, is_site, empty) {
            *slot = value;
        }
    }

    #[cold]
    fn init(
        &mut self,
        insns: &[Insn],
        target_pc: usize,
        is_site: impl Fn(&Insn) -> bool,
        empty: impl Fn() -> T,
    ) {
        let len = insns.len();
        if target_pc >= len {
            return;
        }
        let mut count = 0usize;
        for (i, insn) in insns.iter().enumerate() {
            if i == target_pc || is_site(insn) {
                count += 1;
            }
        }
        if count == 0 {
            return;
        }
        if count <= usize::from(u8::MAX) {
            let mut pc_to_slot = vec![u8::MAX; len];
            let mut next = 0u8;
            for (i, insn) in insns.iter().enumerate() {
                if i == target_pc || is_site(insn) {
                    pc_to_slot[i] = next;
                    next = next.wrapping_add(1);
                }
            }
            let mut entries = Vec::with_capacity(count);
            entries.resize_with(count, empty);
            self.storage = SiteCacheStorage::Small {
                pc_to_slot: pc_to_slot.into_boxed_slice(),
                entries: entries.into_boxed_slice(),
            };
        } else if count <= usize::from(u16::MAX) {
            let mut pc_to_slot = vec![u16::MAX; len];
            let mut next = 0u16;
            for (i, insn) in insns.iter().enumerate() {
                if i == target_pc || is_site(insn) {
                    pc_to_slot[i] = next;
                    next = next.wrapping_add(1);
                }
            }
            let mut entries = Vec::with_capacity(count);
            entries.resize_with(count, empty);
            self.storage = SiteCacheStorage::Medium {
                pc_to_slot: pc_to_slot.into_boxed_slice(),
                entries: entries.into_boxed_slice(),
            };
        } else {
            let mut entries = Vec::with_capacity(len);
            entries.resize_with(len, empty);
            self.storage = SiteCacheStorage::Dense(entries.into_boxed_slice());
        }
    }

    /// Exact heap bytes currently owned by this cache table.
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> usize {
        match &self.storage {
            SiteCacheStorage::Empty => 0,
            SiteCacheStorage::Small {
                pc_to_slot,
                entries,
            } => pc_to_slot.len() + entries.len() * std::mem::size_of::<T>(),
            SiteCacheStorage::Medium {
                pc_to_slot,
                entries,
            } => pc_to_slot.len() * 2 + entries.len() * std::mem::size_of::<T>(),
            SiteCacheStorage::Dense(entries) => entries.len() * std::mem::size_of::<T>(),
        }
    }
}

/// Ultra-sparse per-PC inline cache backed by a PC-sorted `Vec<(u32, T)>`.
///
/// Allocates 0 heap bytes until an entry is actually inserted.
#[derive(Debug, Clone)]
pub(crate) struct SparsePcCache<T> {
    entries: Vec<(u32, T)>,
}

impl<T> SparsePcCache<T> {
    /// Create an empty cache with 0 bytes of heap allocation.
    #[inline]
    pub(crate) const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Look up the entry for `pc`, returning `None` if absent.
    #[inline]
    pub(crate) fn get(&self, pc: usize) -> Option<&T> {
        let pc = u32::try_from(pc).ok()?;
        if self.entries.len() <= 8 {
            for (k, v) in &self.entries {
                if *k == pc {
                    return Some(v);
                }
            }
            None
        } else {
            self.entries
                .binary_search_by_key(&pc, |(k, _)| *k)
                .ok()
                .map(|idx| &self.entries[idx].1)
        }
    }

    /// Insert or replace the entry at `pc`.
    #[inline]
    pub(crate) fn insert(&mut self, pc: usize, value: T) {
        let Ok(pc) = u32::try_from(pc) else {
            return;
        };
        match self.entries.binary_search_by_key(&pc, |(k, _)| *k) {
            Ok(idx) => self.entries[idx].1 = value,
            Err(idx) => self.entries.insert(idx, (pc, value)),
        }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.entries.capacity()
    }

    /// Exact heap bytes currently owned by this sparse cache.
    #[cfg(test)]
    pub(crate) fn heap_bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<(u32, T)>()
    }
}
