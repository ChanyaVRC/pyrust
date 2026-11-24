//! Interned identifier symbols (`Symbol(u32)`).
//!
//! Identifiers in the AST and compiled bytecode repeat heavily (`self`, `x`,
//! `__init__`, `append`, …). Storing each identifier as a 4-byte `Copy`
//! [`Symbol`] eliminates per-occurrence heap `String` allocations, clones, and
//! byte-slice hashing across lexing, parsing, compilation, optimization, and
//! bytecode name tables.
//!
//! ## Concurrency & fast-path design
//! A process-global `OnceLock<RwLock<GlobalInterner>>` is the single source of
//! truth so `Symbol` values are `Send + Sync` and have identical numeric IDs
//! across all threads (including parallel `cargo test` workers). Each thread
//! maintains a monotonic read-only mirror cache in `thread_local!` storage,
//! making repeated `Symbol::intern` and `Symbol::as_str` lookups completely
//! lock-free and allocation-free.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::{OnceLock, RwLock};

use rustc_hash::FxBuildHasher;

/// Compact 4-byte `Copy` handle to a globally interned identifier string.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Symbol(u32);

struct GlobalInterner {
    to_sym: HashMap<&'static str, Symbol, FxBuildHasher>,
    to_str: Vec<&'static str>,
}

impl GlobalInterner {
    fn new() -> Self {
        Self {
            to_sym: HashMap::with_hasher(FxBuildHasher),
            to_str: Vec::with_capacity(256),
        }
    }

    fn intern(&mut self, s: &str) -> Symbol {
        if let Some(&sym) = self.to_sym.get(s) {
            return sym;
        }
        let id = u32::try_from(self.to_str.len()).expect("Symbol table overflow (> u32::MAX)");
        let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
        let sym = Symbol(id);
        self.to_str.push(leaked);
        self.to_sym.insert(leaked, sym);
        sym
    }
}

static GLOBAL_INTERNER: OnceLock<RwLock<GlobalInterner>> = OnceLock::new();

fn global_interner() -> &'static RwLock<GlobalInterner> {
    GLOBAL_INTERNER.get_or_init(|| RwLock::new(GlobalInterner::new()))
}

struct ThreadSymbolCache {
    to_sym: HashMap<&'static str, Symbol, FxBuildHasher>,
    to_str: Vec<&'static str>,
}

impl ThreadSymbolCache {
    fn new() -> Self {
        Self {
            to_sym: HashMap::with_hasher(FxBuildHasher),
            to_str: Vec::with_capacity(256),
        }
    }

    /// Pull any newly interned entries from the global append-only table into
    /// this thread's monotonic mirror.
    fn sync_from_global(&mut self, global: &GlobalInterner) {
        if self.to_str.len() < global.to_str.len() {
            let start = self.to_str.len();
            self.to_str.extend_from_slice(&global.to_str[start..]);
            for (offset, &text) in global.to_str[start..].iter().enumerate() {
                let sym = Symbol((start + offset) as u32);
                self.to_sym.insert(text, sym);
            }
        }
    }
}

// Thread-local justification:
// (a) Why `thread_local!`: Monotonic read-only mirror of the global append-only
//     `Symbol` table so hot-path `Symbol::intern` and `Symbol::as_str` avoid
//     taking `GLOBAL_INTERNER`'s `RwLock` on every identifier/attribute access.
// (b) Multi-interpreter / multi-threaded impact: `Symbol` IDs are assigned
//     solely by the process-global `GLOBAL_INTERNER`, so every thread and
//     interpreter observes identical `Symbol(u32)` IDs and immutable `&'static
//     str` slices; the thread-local mirror is a pure performance cache.
thread_local! {
    static TLS_SYMBOLS: RefCell<ThreadSymbolCache> = RefCell::new(ThreadSymbolCache::new());
}

impl Symbol {
    /// Intern `s` and return its canonical `Symbol`.
    #[inline]
    pub fn intern(s: &str) -> Self {
        TLS_SYMBOLS.with(|cell| {
            if let Some(&sym) = cell.borrow().to_sym.get(s) {
                return sym;
            }
            // Fast shared-read check against the global table first (avoids
            // writer contention when another thread already interned `s`).
            let lock = global_interner();
            {
                let guard = lock.read().unwrap_or_else(|e| e.into_inner());
                if let Some(&sym) = guard.to_sym.get(s) {
                    cell.borrow_mut().sync_from_global(&guard);
                    return sym;
                }
            }
            let mut guard = lock.write().unwrap_or_else(|e| e.into_inner());
            let sym = guard.intern(s);
            cell.borrow_mut().sync_from_global(&guard);
            sym
        })
    }

    /// Resolve this symbol to its immutable interned string slice.
    #[inline]
    pub fn as_str(self) -> &'static str {
        let idx = self.0 as usize;
        TLS_SYMBOLS.with(|cell| {
            if let Some(&text) = cell.borrow().to_str.get(idx) {
                return text;
            }
            let guard = global_interner().read().unwrap_or_else(|e| e.into_inner());
            let mut cache = cell.borrow_mut();
            cache.sync_from_global(&guard);
            cache.to_str[idx]
        })
    }

    /// Return the underlying compact `u32` identifier.
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl Deref for Symbol {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for Symbol {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl PartialEq<str> for Symbol {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Symbol {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for Symbol {
    #[inline]
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<Symbol> for str {
    #[inline]
    fn eq(&self, other: &Symbol) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<Symbol> for &str {
    #[inline]
    fn eq(&self, other: &Symbol) -> bool {
        *self == other.as_str()
    }
}

impl PartialEq<Symbol> for String {
    #[inline]
    fn eq(&self, other: &Symbol) -> bool {
        self.as_str() == other.as_str()
    }
}

impl From<&str> for Symbol {
    #[inline]
    fn from(s: &str) -> Self {
        Self::intern(s)
    }
}

impl From<String> for Symbol {
    #[inline]
    fn from(s: String) -> Self {
        Self::intern(&s)
    }
}

impl From<&String> for Symbol {
    #[inline]
    fn from(s: &String) -> Self {
        Self::intern(s)
    }
}

impl From<Symbol> for String {
    #[inline]
    fn from(sym: Symbol) -> Self {
        sym.as_str().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::Symbol;

    #[test]
    fn symbol_roundtrip_dedup_and_traits() {
        let a = Symbol::intern("pyrust_sym_alpha");
        let b = Symbol::intern("pyrust_sym_alpha");
        let c = Symbol::intern("pyrust_sym_beta");

        assert_eq!(a, b);
        assert_eq!(a.as_u32(), b.as_u32());
        assert_ne!(a, c);
        assert_eq!(a.as_str(), "pyrust_sym_alpha");
        assert_eq!(a, "pyrust_sym_alpha");
        assert_eq!("pyrust_sym_alpha", a);
        assert_eq!(format!("{a}"), "pyrust_sym_alpha");
        assert_eq!(format!("{a:?}"), "\"pyrust_sym_alpha\"");
        assert_eq!(std::mem::size_of::<Symbol>(), 4);

        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
        assert!(!set.contains(&c));

        // Copy semantics and From conversions.
        let copy_a: Symbol = a;
        assert_eq!(copy_a, Symbol::from(String::from("pyrust_sym_alpha")));
        assert_eq!(String::from(copy_a), "pyrust_sym_alpha");
    }

    #[test]
    fn symbol_is_send_sync_and_consistent_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Symbol>();

        let handles: Vec<_> = (0..8)
            .map(|tid| {
                std::thread::spawn(move || {
                    let mut out = Vec::new();
                    for i in 0..64 {
                        let shared = Symbol::intern(&format!("shared_sym_{i}"));
                        let unique = Symbol::intern(&format!("thread_{tid}_sym_{i}"));
                        assert_eq!(shared.as_str(), format!("shared_sym_{i}"));
                        assert_eq!(unique.as_str(), format!("thread_{tid}_sym_{i}"));
                        out.push((shared, unique));
                    }
                    out
                })
            })
            .collect();

        let results: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("worker thread panicked"))
            .collect();

        // Every thread must observe the exact same Symbol ID for every shared name,
        // and the main thread must resolve symbols created on every worker thread.
        for i in 0..64 {
            let expected = Symbol::intern(&format!("shared_sym_{i}"));
            for (tid, worker_syms) in results.iter().enumerate() {
                let (shared, unique) = worker_syms[i];
                assert_eq!(shared, expected);
                assert_eq!(unique.as_str(), format!("thread_{tid}_sym_{i}"));
            }
        }
    }
}
