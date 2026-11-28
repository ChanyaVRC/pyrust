//! Compile-time registry of built-in Python callables.
//!
//! Built-ins are declared in one of two ways (see the `pyrust-derive` crate):
//!
//! - **`pyrust_module! { … }`** (function-like, primary) — declares a
//!   whole built-in module's callables and constants at once.  Each `fn`
//!   inside the macro expands to a `BuiltinReg` and is collected into the
//!   per-module `REGS` slice.  Function names that collide with Rust
//!   keywords use the `#[py_name = "..."]` override.
//! - **`#[pyfunction(name = "module.fn")]`** (attribute, one-off
//!   fallback) — for migrating a single arm without spinning up a module
//!   file.
//!
//! Every module's registrations are a `static REGS: &[BuiltinReg]` slice in
//! read-only data, sorted by name when the macro expands.  This module
//! resolves a name against those slices in place, in two tiers:
//!
//! - **Tier 1** — `builtins` and the shared `_structseq` hooks, merged into
//!   one sorted table at compile time.  It is the only tier a bare
//!   interpreter needs, and looking a name up in it reads nothing else.
//! - **Tier 2** — every other module.  A dotted name is resolved by finding
//!   the module that owns its prefix (`os.path.join` tries `os.path`, then
//!   `os`) in a table of module names sorted at compile time, then
//!   binary-searching that module's own slice.  A module's slice is first
//!   read when one of its callables is looked up; importing the module does
//!   not read it.  Until then it costs no page faults and no heap.
//!
//! Compared to the previous `match ValueKind::BuiltinFunction("name") => …`
//! cascade, this design:
//! - decouples the *declaration* of a built-in from its dispatch arm,
//! - dispatches through a `fn` pointer instead of a string match,
//! - allocates nothing, at start-up or per call: both tiers are `&'static`
//!   and are read without locks.
//!
//! Callers almost always ask with the registry's own `&'static str` (it is
//! what `Value::BuiltinFunction` carries), so a small lock-free memo keyed by
//! that pointer answers a repeated lookup without searching either tier.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use crate::Interpreter;
use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;

/// Unified dispatch signature for built-in callables.
///
/// `interp` gives access to the interpreter for built-ins that re-enter
/// (e.g. `map`, `reduce`, `sorted` with a key).  Pure built-ins like
/// `abs` or `math.sqrt` ignore it.
pub type BuiltinDispatchFn = fn(&mut Interpreter, &[ExpandedCallArg]) -> Result<Value>;

/// "Vectorcall"-style fast dispatch for a typed built-in with no `*args` /
/// `**kwargs` / keyword-only parameters: the caller (a positional `Insn::Call`)
/// passes the argument *values* directly as a slice, so the fast entry skips
/// the `ExpandedCallArg` buffer, the keyword-argument validation, and the arity
/// pre-check that the general [`BuiltinDispatchFn`] pays on every call. The
/// caller guarantees `min_arity <= args.len() <= max_arity`.
pub type BuiltinFastDispatchFn = fn(&mut Interpreter, &[Value]) -> Result<Value>;

/// Python-visible callable category carried by a registry entry.
///
/// A dot in the internal dispatch key is not semantic: both a module function
/// (`math.sqrt`) and an unbound type method (`list.append`) use dotted keys.
/// Consumers must use this tag instead of reparsing [`BuiltinReg::name`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BuiltinCallableKind {
    ModuleFunction,
    MethodDescriptor,
}

/// Presentation metadata for a registered callable.
///
/// `qualname` is the name declared inside `pyrust_module!`: `sqrt` for a
/// module function and `Counter.__getitem__` for a type member.  Some legacy
/// flat-builtin declarations include the module prefix in that string
/// (`builtins.TypeAliasType.__repr__`); accessors normalize it without
/// allocating. `declaring_module` remains available for module-function
/// `__module__` and for diagnostics.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BuiltinCallableMetadata {
    pub kind: BuiltinCallableKind,
    pub declaring_module: &'static str,
    pub qualname: &'static str,
    canonical_owner: Option<pyrust_core::CanonicalClassTag>,
}

impl BuiltinCallableMetadata {
    pub const fn module_function(declaring_module: &'static str, qualname: &'static str) -> Self {
        Self {
            kind: BuiltinCallableKind::ModuleFunction,
            declaring_module,
            qualname,
            canonical_owner: None,
        }
    }

    pub const fn method_descriptor(declaring_module: &'static str, qualname: &'static str) -> Self {
        Self {
            kind: BuiltinCallableKind::MethodDescriptor,
            declaring_module,
            qualname,
            canonical_owner: None,
        }
    }

    /// Construct a descriptor declared by an interpreter-owned canonical
    /// class.  The derive macro emits this form for primitive/object owners so
    /// slot dispatch can validate receivers without reparsing a qualified
    /// callable name.
    pub const fn canonical_method_descriptor(
        declaring_module: &'static str,
        qualname: &'static str,
        owner: pyrust_core::CanonicalClassTag,
    ) -> Self {
        Self {
            kind: BuiltinCallableKind::MethodDescriptor,
            declaring_module,
            qualname,
            canonical_owner: Some(owner),
        }
    }

    /// Python `__qualname__`, excluding a redundant registration-module prefix.
    pub fn python_qualname(self) -> &'static str {
        if self.kind == BuiltinCallableKind::MethodDescriptor
            && self.qualname.starts_with(self.declaring_module)
            && self
                .qualname
                .as_bytes()
                .get(self.declaring_module.len())
                .is_some_and(|separator| *separator == b'.')
        {
            &self.qualname[self.declaring_module.len() + 1..]
        } else {
            self.qualname
        }
    }

    /// Python `__name__`, derived from the declared qualname.
    pub fn python_name(self) -> &'static str {
        self.python_qualname()
            .rsplit_once('.')
            .map_or(self.python_qualname(), |(_, name)| name)
    }

    /// Python `__module__`; method descriptors do not expose this attribute.
    pub const fn python_module(self) -> Option<&'static str> {
        match self.kind {
            BuiltinCallableKind::ModuleFunction => Some(self.declaring_module),
            BuiltinCallableKind::MethodDescriptor => None,
        }
    }

    /// Descriptor owner (`list`, `Counter`, …), when this is a type member.
    #[allow(dead_code)] // Public metadata API; most dispatchers need the typed tag below.
    pub fn owner(self) -> Option<&'static str> {
        match self.kind {
            BuiltinCallableKind::ModuleFunction => None,
            BuiltinCallableKind::MethodDescriptor => self
                .python_qualname()
                .rsplit_once('.')
                .map(|(owner, _)| owner),
        }
    }

    /// Immutable class identity for primitive/object descriptors.
    ///
    /// Named non-canonical owners such as `Counter` intentionally return
    /// `None`; callers that only need Python presentation should use
    /// [`Self::owner`].
    pub const fn descriptor_owner_tag(self) -> Option<pyrust_core::CanonicalClassTag> {
        match self.kind {
            BuiltinCallableKind::ModuleFunction => None,
            BuiltinCallableKind::MethodDescriptor => self.canonical_owner,
        }
    }
}

/// One entry in the registry — emitted by `pyrust_module!` (one per fn
/// inside the macro body) or by `#[pyfunction(name = …)]`.
///
/// Built entirely from constants, so a slice of these is a `static` in
/// read-only data.
#[derive(Copy, Clone)]
pub struct BuiltinReg {
    pub name: &'static str,
    pub dispatch: BuiltinDispatchFn,
    pub metadata: BuiltinCallableMetadata,
    /// Optional fast entry for a positional call (see [`BuiltinFastDispatchFn`]).
    /// `None` for legacy-dialect builtins and any typed builtin with `*args` /
    /// `**kwargs` / keyword-only params. Valid only for `min_arity..=max_arity`
    /// positional arguments.
    pub fast: Option<BuiltinFastDispatchFn>,
    pub min_arity: u8,
    pub max_arity: u8,
}

/// Copy a registry name into an array, so `pyrust_module!` can give each
/// name's bytes a `static` of their own (see [`name_str`]).
pub const fn name_bytes<const N: usize>(name: &str) -> [u8; N] {
    let source = name.as_bytes();
    assert!(source.len() == N, "name length does not match its storage");
    let mut bytes = [0u8; N];
    let mut i = 0;
    while i < N {
        bytes[i] = source[i];
        i += 1;
    }
    bytes
}

/// View a name's `static` bytes as the `&'static str` every user of that
/// name shares.
///
/// The string points into the `static`, which has a single address in the
/// program, so two uses of the same name are pointer-equal.  That is the
/// identity `Value::BuiltinFunction(&'static str)` relies on.
pub const fn name_str(bytes: &'static [u8]) -> &'static str {
    match std::str::from_utf8(bytes) {
        Ok(name) => name,
        Err(_) => panic!("registry name is not valid UTF-8"),
    }
}

/// One lazily registered module: its name and its sorted `REGS` slice.
///
/// `name` is the dotted prefix every registration in `regs` carries
/// (`"os.path"` for `os.path.join`), which is also the module's import name.
#[derive(Copy, Clone)]
pub struct ModuleRegs {
    pub name: &'static str,
    pub regs: &'static [BuiltinReg],
}

/// Bytewise `a < b`, usable in constant evaluation.  Matches `str`'s `Ord`.
const fn name_lt(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut i = 0;
    while i < a.len() && i < b.len() {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
        i += 1;
    }
    a.len() < b.len()
}

/// Generate a pair of `const fn`s that concatenate slices of a `Copy` row
/// type with a `name: &'static str` field into one array sorted by that name:
/// `$count_fn` gives the array length, `$sort_fn` builds it.  Rows for which
/// `keep` is false are left out.
///
/// The result is evaluated at compile time, so the table it initialises is a
/// constant in read-only data.  A repeated name fails the build, naming the
/// offender: a duplicate would make lookups ambiguous.
macro_rules! const_sorted_by_name {
    (
        $(#[$meta:meta])*
        const fn $sort_fn:ident, $count_fn:ident -> [$row:ty],
        filler = $filler:expr,
        keep = |$kept:ident| $keep:expr $(,)?
    ) => {
        /// Number of rows the matching sort function produces.
        const fn $count_fn(parts: &[&[$row]], extra: &[$row]) -> usize {
            let mut total = 0;
            let mut part = 0;
            while part <= parts.len() {
                let source = if part < parts.len() {
                    parts[part]
                } else {
                    extra
                };
                let mut i = 0;
                while i < source.len() {
                    let $kept = &source[i];
                    if $keep {
                        total += 1;
                    }
                    i += 1;
                }
                part += 1;
            }
            total
        }

        $(#[$meta])*
        const fn $sort_fn<const N: usize>(parts: &[&[$row]], extra: &[$row]) -> [$row; N] {
            const fn sift_down<const N: usize>(rows: &mut [$row; N], mut root: usize, end: usize) {
                loop {
                    let mut child = 2 * root + 1;
                    if child >= end {
                        return;
                    }
                    if child + 1 < end && name_lt(rows[child].name, rows[child + 1].name) {
                        child += 1;
                    }
                    if !name_lt(rows[root].name, rows[child].name) {
                        return;
                    }
                    rows.swap(root, child);
                    root = child;
                }
            }

            let mut rows: [$row; N] = [$filler; N];
            let mut len = 0;
            let mut part = 0;
            while part <= parts.len() {
                let source = if part < parts.len() {
                    parts[part]
                } else {
                    extra
                };
                let mut i = 0;
                while i < source.len() {
                    let $kept = &source[i];
                    if $keep {
                        rows[len] = source[i];
                        len += 1;
                    }
                    i += 1;
                }
                part += 1;
            }
            assert!(len == N, "row count does not match the table length");

            // Heapsort: O(n log n) keeps constant evaluation cheap.
            let mut start = N / 2;
            while start > 0 {
                start -= 1;
                sift_down(&mut rows, start, N);
            }
            let mut end = N;
            while end > 1 {
                end -= 1;
                rows.swap(0, end);
                sift_down(&mut rows, 0, end);
            }

            let mut i = 1;
            while i < N {
                if !name_lt(rows[i - 1].name, rows[i].name) {
                    // Constant panics cannot format; the message is the
                    // duplicated registry name.
                    panic!("{}", rows[i].name);
                }
                i += 1;
            }
            rows
        }
    };
}

/// Placeholder dispatch for table slots before they are filled; never
/// reachable through a lookup.
fn unfilled_dispatch(_: &mut Interpreter, _: &[ExpandedCallArg]) -> Result<Value> {
    unreachable!("registry table slot was never filled")
}

const_sorted_by_name! {
    /// Merge registration slices into one table sorted by callable name.
    const fn sorted_regs, count_regs -> [BuiltinReg],
    filler = BuiltinReg {
        name: "",
        dispatch: unfilled_dispatch,
        metadata: BuiltinCallableMetadata::module_function("", ""),
        fast: None,
        min_arity: 0,
        max_arity: 0,
    },
    keep = |_registration| true,
}

const_sorted_by_name! {
    /// Merge module tables into one table sorted by module name.  A module
    /// with no native callables (a pure-Python body, or the flat `builtins`
    /// module, whose callables are tier 1) has nothing to resolve and gets
    /// no row.
    const fn sorted_modules, count_modules -> [ModuleRegs],
    filler = ModuleRegs { name: "", regs: &[] },
    keep = |module| !module.regs.is_empty(),
}

const TIER1_LEN: usize = count_regs(
    crate::builtin_modules::TIER1_REG_FAMILIES,
    crate::builtin_modules::TIER1_EXTRA_REGS,
);

/// Tier 1: every callable a bare interpreter can reach before any import —
/// the `builtins` families plus the shared `_structseq` hooks.  Sorted and
/// checked for duplicates at compile time.
static TIER1: [BuiltinReg; TIER1_LEN] = sorted_regs(
    crate::builtin_modules::TIER1_REG_FAMILIES,
    crate::builtin_modules::TIER1_EXTRA_REGS,
);

/// The docstring of a native-Rust callable (`cmd.Cmd.do_help`), by registry
/// name: `None` when no docs are registered for it, `Some(None)` when it is
/// registered and has no docstring.
pub(crate) fn native_callable_doc(registry_key: &str) -> Option<Option<&'static str>> {
    crate::builtin_modules::native_callable_doc(registry_key)
}

const MODULE_COUNT: usize = count_modules(crate::builtin_modules::MODULE_REG_TABLES, &[]);

/// Tier 2: one row per built-in module that registers native callables,
/// sorted by module name at compile time.  A row only *points* at the
/// module's `REGS`; the slice itself is not read until one of the module's
/// callables is looked up.
///
/// Adding a module touches only the `pyrust_builtin_modules!` invocation in
/// `builtin_modules/mod.rs` — this file needs no edits.
static MODULES: [ModuleRegs; MODULE_COUNT] =
    sorted_modules(crate::builtin_modules::MODULE_REG_TABLES, &[]);

/// Whether each [`MODULES`] row has been registered (validated and counted).
static MODULE_REGISTERED: [AtomicBool; MODULE_COUNT] =
    [const { AtomicBool::new(false) }; MODULE_COUNT];

/// Number of tier-2 modules registered so far, and the callables they hold.
static LAZY_MODULES: AtomicUsize = AtomicUsize::new(0);
static LAZY_CALLABLES: AtomicUsize = AtomicUsize::new(0);

// Thread-local justification:
// (a) Why `thread_local!`: test-only log of the tier-2 modules whose slices
//     the current thread consulted, so a test can assert that a code path
//     reads no un-imported module without being perturbed by tests running
//     on other threads (the process-wide flags above are shared).
// (b) Multi-interpreter / multi-threaded impact: none outside tests.
#[cfg(test)]
thread_local! {
    static CONSULTED_MODULES: std::cell::RefCell<std::collections::BTreeSet<&'static str>> =
        const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
}

#[inline]
fn find_index(regs: &[BuiltinReg], name: &str) -> Option<usize> {
    regs.binary_search_by(|registration| registration.name.cmp(name))
        .ok()
}

#[inline]
fn find_module(name: &str) -> Option<usize> {
    MODULES
        .binary_search_by(|module| module.name.cmp(name))
        .ok()
}

#[cfg(test)]
fn note_consulted(module_index: usize) {
    CONSULTED_MODULES.with(|consulted| {
        consulted.borrow_mut().insert(MODULES[module_index].name);
    });
}

/// The registrations of the module at `index`, registering the module if
/// this is its first use.
#[inline]
fn module_slice(index: usize) -> &'static [BuiltinReg] {
    #[cfg(test)]
    note_consulted(index);
    if !MODULE_REGISTERED[index].load(Ordering::Relaxed) {
        register_module_at(index);
    }
    MODULES[index].regs
}

/// The registrations of built-in module `name`, registering the module if
/// this is its first use.  Returns `None` if no built-in module with native
/// callables has that name.
///
/// Rows come from the tables every module list generates (see
/// `crate::builtin_modules::MODULE_REG_TABLES`), so a newly declared module
/// is found here with no further wiring.
#[allow(dead_code)] // Lookups go through `locate`; this is the by-module entry point.
pub(crate) fn module_regs(name: &str) -> Option<&'static [BuiltinReg]> {
    find_module(name).map(module_slice)
}

/// Validate a module's slice and record it as registered.
///
/// This runs once per module, the first time one of its callables is looked
/// up — importing the module does not read its slice.  Concurrent first uses
/// may both validate; only one is counted.
#[cold]
fn register_module_at(index: usize) {
    let module = &MODULES[index];
    validate_module(module);
    if !MODULE_REGISTERED[index].swap(true, Ordering::Relaxed) {
        LAZY_MODULES.fetch_add(1, Ordering::Relaxed);
        LAZY_CALLABLES.fetch_add(module.regs.len(), Ordering::Relaxed);
    }
}

/// Panic unless every registration of `module` can be resolved by a lookup.
///
/// `assert!` (not `debug_assert!`): lookups find the module by the dotted
/// prefix of a name and then binary-search its slice, so a name outside the
/// module's prefix, an unsorted slice, or a duplicate would make `lookup()`
/// silently miss or ambiguous in release builds.
fn validate_module(module: &ModuleRegs) {
    for registration in module.regs {
        assert!(
            registration
                .name
                .strip_prefix(module.name)
                .is_some_and(|rest| rest.starts_with('.')),
            "built-in `{}` is registered by module `{}` but is not named `{}.<name>`",
            registration.name,
            module.name,
            module.name,
        );
    }
    if let Some(pair) = module
        .regs
        .windows(2)
        .find(|pair| pair[0].name >= pair[1].name)
    {
        panic!(
            "duplicate or unsorted built-in name in module `{}`: `{}` is followed by `{}`",
            module.name, pair[0].name, pair[1].name,
        );
    }
}

/// Where a registration lives: `table` 0 is [`TIER1`], and `table` `n` is
/// the slice of `MODULES[n - 1]`.
#[derive(Copy, Clone)]
struct Location {
    table: usize,
    index: usize,
}

impl Location {
    /// The registration at this location, if the location is in range.
    #[inline]
    fn registration(self) -> Option<&'static BuiltinReg> {
        let regs: &'static [BuiltinReg] = match self.table.checked_sub(1) {
            None => &TIER1,
            Some(module) => MODULES.get(module)?.regs,
        };
        regs.get(self.index)
    }

    /// Pack into a non-zero memo word, or `None` if it does not fit.
    #[inline]
    fn pack(self) -> Option<u32> {
        let table = u16::try_from(self.table).ok()?;
        let index = u16::try_from(self.index).ok()?;
        // `+ 1` keeps zero free to mean "empty slot".
        ((u32::from(table) << 16) | u32::from(index)).checked_add(1)
    }

    #[inline]
    fn unpack(word: u32) -> Option<Self> {
        let word = word.checked_sub(1)?;
        Some(Self {
            table: (word >> 16) as usize,
            index: (word & 0xFFFF) as usize,
        })
    }
}

/// Find `name` by searching the tiers.
///
/// Tier 1 is searched first, so `builtins` names never consult a module.
/// Otherwise the name is resolved through the module that owns its dotted
/// prefix, longest prefix first (`a.b.c.f` → `a.b.c`, `a.b`, `a`), so a
/// callable reached through a submodule, a re-exporting shim, or a class
/// (`os.stat_result.__repr__`) resolves to the module that registered it,
/// whether or not that module was ever imported under this name.  Only
/// modules that own one of the prefixes are consulted; an undotted name
/// consults none.
fn locate(name: &str) -> Option<Location> {
    if let Some(index) = find_index(&TIER1, name) {
        return Some(Location { table: 0, index });
    }
    let mut prefix = name;
    // `.` is ASCII, so the split always lands on a character boundary.
    while let Some(dot) = prefix.bytes().rposition(|byte| byte == b'.') {
        prefix = &prefix[..dot];
        if let Some(module) = find_module(prefix)
            && let Some(index) = find_index(module_slice(module), name)
        {
            return Some(Location {
                table: module + 1,
                index,
            });
        }
    }
    None
}

const MEMO_BITS: u32 = 9;

/// Memo of recent lookups, keyed by the address of the queried name.
///
/// Each slot holds a packed [`Location`] (zero when empty).  A hit is
/// accepted only if the registration found there has *the same* `&'static
/// str` as the query — same address and length — which proves the query is
/// that registration's name without comparing bytes.  A query made with any
/// other string (a formatted temporary, say) simply misses and takes the
/// search.  Slots are independent words, so concurrent readers and writers
/// need no lock: a racing store only replaces one valid entry with another.
static MEMO: [AtomicU32; 1 << MEMO_BITS] = [const { AtomicU32::new(0) }; 1 << MEMO_BITS];

#[inline]
fn memo_slot(name: &str) -> &'static AtomicU32 {
    // Fibonacci hashing: registry names sit next to each other in read-only
    // data, so the multiply spreads their addresses across the slots.
    let hash = (name.as_ptr() as usize as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    &MEMO[(hash >> (u64::BITS - MEMO_BITS)) as usize]
}

/// The registration a memo word stands for, if it is the one named `name`.
#[inline]
fn recall(word: u32, name: &str) -> Option<&'static BuiltinReg> {
    let location = Location::unpack(word)?;
    let registration = location.registration()?;
    if !std::ptr::eq(registration.name, name) {
        return None;
    }
    #[cfg(test)]
    if let Some(module) = location.table.checked_sub(1) {
        note_consulted(module);
    }
    Some(registration)
}

/// Search for `name` and remember where it was found.
fn lookup_uncached(name: &str, slot: &AtomicU32) -> Option<&'static BuiltinReg> {
    let location = locate(name)?;
    let registration = location.registration()?;
    // Only the registry's own string can be recognised by address later.
    if std::ptr::eq(registration.name, name)
        && let Some(word) = location.pack()
    {
        slot.store(word, Ordering::Relaxed);
    }
    Some(registration)
}

/// How much of the registry the process has used so far.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) struct RegistryStats {
    /// Callables in the always-available tier (`builtins` + `_structseq`).
    pub tier1_callables: usize,
    /// Tier-2 modules registered so far.
    pub lazy_modules: usize,
    /// Callables those modules hold.
    pub lazy_callables: usize,
    /// Tier-2 modules that exist (those with native callables), registered
    /// or not.
    pub known_modules: usize,
    /// Callables those modules declare in total.  Counted from the slice
    /// lengths alone, so reporting it reads no module's registrations.
    pub known_callables: usize,
}

pub(crate) fn stats() -> RegistryStats {
    RegistryStats {
        tier1_callables: TIER1.len(),
        lazy_modules: LAZY_MODULES.load(Ordering::Relaxed),
        lazy_callables: LAZY_CALLABLES.load(Ordering::Relaxed),
        known_modules: MODULES.len(),
        known_callables: MODULES.iter().map(|module| module.regs.len()).sum(),
    }
}

/// Names of the tier-2 modules registered so far, in sorted order.
pub(crate) fn registered_modules() -> Vec<&'static str> {
    MODULES
        .iter()
        .zip(&MODULE_REGISTERED)
        .filter(|(_, registered)| registered.load(Ordering::Relaxed))
        .map(|(module, _)| module.name)
        .collect()
}

/// Environment variable that makes the CLI report registry usage on exit.
pub(crate) const STATS_ENV_VAR: &str = "PYRUST_REGISTRY_STATS";

/// Print registry usage to stderr when [`STATS_ENV_VAR`] is set to a
/// non-empty value.  A diagnostic for measuring start-up cost: it shows which
/// modules' callables a run actually registered.
pub(crate) fn report_stats_if_requested() {
    if std::env::var_os(STATS_ENV_VAR).is_none_or(|value| value.is_empty()) {
        return;
    }
    let stats = stats();
    eprintln!(
        "pyrust-registry: tier1_callables={} lazy_modules={} lazy_callables={} known_modules={} known_callables={} modules=[{}]",
        stats.tier1_callables,
        stats.lazy_modules,
        stats.lazy_callables,
        stats.known_modules,
        stats.known_callables,
        registered_modules().join(","),
    );
}

/// Look up the dispatcher for a Python-level built-in name.  Returns
/// `None` if no built-in by that name is registered, in which case the
/// caller falls back to the legacy match cascade.
#[inline]
pub fn lookup(name: &str) -> Option<BuiltinDispatchFn> {
    lookup_registration(name).map(|registration| registration.dispatch)
}

/// Look up the complete immutable registration for a built-in callable.
#[inline]
pub fn lookup_registration(name: &str) -> Option<&'static BuiltinReg> {
    let slot = memo_slot(name);
    match recall(slot.load(Ordering::Relaxed), name) {
        Some(registration) => Some(registration),
        None => lookup_uncached(name, slot),
    }
}

/// Look up typed presentation/ownership metadata for a built-in callable.
#[inline]
pub fn lookup_metadata(name: &str) -> Option<BuiltinCallableMetadata> {
    lookup_registration(name).map(|registration| registration.metadata)
}

/// Look up the interned `&'static str` name for a registered built-in.
///
/// The registry stores the canonical `&'static str` for each built-in (see
/// [`BuiltinReg::name`]).  This function returns that pointer so callers can
/// construct a `Value::builtin_function(name)` without allocating a new
/// `&'static str` from a heap-leaked `Box<str>`.
///
/// Returns `None` if no built-in by that name is registered.
#[inline]
pub fn lookup_name(name: &str) -> Option<&'static str> {
    lookup_registration(name).map(|registration| registration.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::ValueKind;

    fn module_attr(module: &Value, name: &str) -> Value {
        let ValueKind::PyModule(module) = module.kind() else {
            panic!("expected PyModule");
        };
        module
            .borrow()
            .attrs
            .get(name)
            .unwrap_or_else(|| panic!("module lacks {name:?}"))
            .clone()
    }

    /// Run `action` and report which tier-2 modules this thread consulted
    /// while it ran.
    fn consulted_during<R>(action: impl FnOnce() -> R) -> (R, Vec<&'static str>) {
        CONSULTED_MODULES.with(|consulted| consulted.borrow_mut().clear());
        let result = action();
        let consulted =
            CONSULTED_MODULES.with(|consulted| consulted.borrow().iter().copied().collect());
        (result, consulted)
    }

    /// A tier-2 row by module name, read without consulting it.
    fn module_row(name: &str) -> &'static ModuleRegs {
        MODULES
            .iter()
            .find(|module| module.name == name)
            .unwrap_or_else(|| panic!("no registry row for module {name:?}"))
    }

    const fn test_reg(name: &'static str) -> BuiltinReg {
        BuiltinReg {
            name,
            dispatch: unfilled_dispatch,
            metadata: BuiltinCallableMetadata::module_function("m", "f"),
            fast: None,
            min_arity: 0,
            max_arity: 0,
        }
    }

    #[test]
    fn lookup_finds_a_known_builtin() {
        // math.sqrt is one of the first migrated builtins; it must be present.
        assert!(
            lookup("math.sqrt").is_some(),
            "math.sqrt should be registered"
        );
    }

    #[test]
    fn lookup_misses_an_unknown_name() {
        assert!(lookup("absolutely-not-a-builtin").is_none());
    }

    #[test]
    fn callable_metadata_distinguishes_module_functions_and_descriptors() {
        let sqrt = lookup_metadata("math.sqrt").expect("math.sqrt must be registered");
        assert_eq!(sqrt.kind, BuiltinCallableKind::ModuleFunction);
        assert_eq!(sqrt.python_name(), "sqrt");
        assert_eq!(sqrt.python_qualname(), "sqrt");
        assert_eq!(sqrt.python_module(), Some("math"));
        assert_eq!(sqrt.owner(), None);
        assert_eq!(sqrt.descriptor_owner_tag(), None);

        let getitem =
            lookup_metadata("list.__getitem__").expect("list.__getitem__ must be registered");
        assert_eq!(getitem.kind, BuiltinCallableKind::MethodDescriptor);
        assert_eq!(getitem.python_name(), "__getitem__");
        assert_eq!(getitem.python_qualname(), "list.__getitem__");
        assert_eq!(getitem.python_module(), None);
        assert_eq!(getitem.owner(), Some("list"));
        assert_eq!(
            getitem.descriptor_owner_tag(),
            Some(pyrust_core::CanonicalClassTag::List)
        );

        // Undotted typing `__class_getitem__` helpers carry explicit
        // `#[method_descriptor]` metadata rather than relying on a dot in
        // their registry key (#2960).
        for cgi in [
            "typing._generic_cgi",
            "typing._protocol_cgi",
            "typing._union_cgi",
            "typing._optional_cgi",
        ] {
            let meta = lookup_metadata(cgi).unwrap_or_else(|| panic!("{cgi} must be registered"));
            assert_eq!(meta.kind, BuiltinCallableKind::MethodDescriptor, "{cgi}");
            assert_eq!(meta.python_module(), None, "{cgi}");
        }

        // Synthetic stdlib class methods register as method descriptors, while
        // plain module functions remain `ModuleFunction`.
        for desc in [
            "sys.version_info.__repr__",
            "sys.version_info.__len__",
            "os.stat_result.__repr__",
            "os.terminal_size.__repr__",
        ] {
            let meta = lookup_metadata(desc).unwrap_or_else(|| panic!("{desc} must be registered"));
            assert_eq!(meta.kind, BuiltinCallableKind::MethodDescriptor, "{desc}");
        }
        for func in ["format", "len", "math.floor", "functools.reduce"] {
            let meta = lookup_metadata(func).unwrap_or_else(|| panic!("{func} must be registered"));
            assert_eq!(meta.kind, BuiltinCallableKind::ModuleFunction, "{func}");
        }
    }

    #[test]
    fn registry_is_sorted_and_unique() {
        // Global uniqueness, over every registration of every module: the
        // lazy tiers are only ever checked one table at a time.
        let mut names: Vec<&'static str> = crate::builtin_modules::all_regs()
            .iter()
            .map(|registration| registration.name)
            .collect();
        names.sort_unstable();
        for w in names.windows(2) {
            assert!(
                w[0] < w[1],
                "built-in name registered more than once: {:?}",
                w[0]
            );
        }

        // Every table a lookup binary-searches is strictly sorted.
        for w in TIER1.windows(2) {
            assert!(
                w[0].name < w[1].name,
                "tier 1 not sorted at {:?}",
                w[0].name
            );
        }
        for w in MODULES.windows(2) {
            assert!(
                w[0].name < w[1].name,
                "module table not sorted at {:?}",
                w[0].name
            );
        }
        for module in &MODULES {
            for w in module.regs.windows(2) {
                assert!(
                    w[0].name < w[1].name,
                    "`{}` registrations not sorted at {:?} >= {:?}",
                    module.name,
                    w[0].name,
                    w[1].name
                );
            }
        }
    }

    #[test]
    fn every_registration_resolves_through_the_lazy_lookup() {
        // Nothing is indexed up front, so each name must be reachable from
        // its own text: tier 1 directly, everything else through the module
        // that owns its dotted prefix.  A registration whose name did not
        // start with its module's name would be silently unreachable.
        let all = crate::builtin_modules::all_regs();
        assert_eq!(
            all.len(),
            TIER1.len()
                + MODULES
                    .iter()
                    .map(|module| module.regs.len())
                    .sum::<usize>(),
            "a registration is in neither tier"
        );
        for registration in &all {
            let found = lookup_registration(registration.name)
                .unwrap_or_else(|| panic!("{:?} does not resolve", registration.name));
            assert!(
                std::ptr::eq(found.name, registration.name),
                "{:?} resolved to a different registration",
                registration.name
            );
        }
    }

    #[test]
    fn flat_namespace_builtins_register_without_prefix() {
        // `@flat builtins` mode means top-level Python builtins
        // (`abs`, `len`, `print`, …) register under their short name,
        // not `builtins.abs`.  Picking a handful at random pins the contract.
        for name in ["abs", "len", "print", "type", "super"] {
            assert!(
                lookup(name).is_some(),
                "expected flat builtin {name:?} in registry"
            );
            assert!(
                lookup(&format!("builtins.{name}")).is_none(),
                "flat builtin {name:?} must NOT be registered with `builtins.` prefix"
            );
        }
    }

    #[test]
    fn registry_name_and_module_attr_share_one_pointer() {
        // `pyrust_module!` stores each built-in's name in one `static`, read
        // by the registry entry, by the dispatch function's `FN_NAME`, and by
        // the `Value::builtin_function(...)` bound into the module.  The
        // `&'static str` stored in the registry must therefore be **the
        // exact same pointer** as the one bound into the imported `PyModule`.
        // Compare with `std::ptr::eq` — string equality alone wouldn't catch
        // a regression to one copy of the name per use.
        let registered = lookup_registration("math.sqrt")
            .expect("math.sqrt must be registered")
            .name;

        let module =
            crate::builtin_modules::load_builtin_module("math").expect("import math must resolve");
        let module_attr_name: &'static str = match module.kind() {
            ValueKind::PyModule(m) => match m.borrow().attrs.get("sqrt").map(|v| v.kind()) {
                Some(ValueKind::BuiltinFunction(s)) => s,
                _ => panic!("math.sqrt attr is not a BuiltinFunction"),
            },
            _ => panic!("import math did not return a PyModule"),
        };

        assert!(
            std::ptr::eq(registered, module_attr_name),
            "BuiltinReg.name and PyModule attr name must point to the same \
             str (got {registered:p} vs {module_attr_name:p})",
        );
    }

    #[test]
    fn module_generation_owns_native_callable_objects() {
        // The derive layer owns the lifetime policy: ordinary native modules
        // get one callable object per generated module, while `@flat
        // builtins` intentionally reuses the process/thread-wide objects
        // installed in the global namespace. The registry dispatch name
        // remains shared in both cases.
        let math_a =
            crate::builtin_modules::load_builtin_module("math").expect("first math module");
        let math_b =
            crate::builtin_modules::load_builtin_module("math").expect("second math module");
        let sqrt_a = module_attr(&math_a, "sqrt");
        let sqrt_b = module_attr(&math_b, "sqrt");
        assert_ne!(sqrt_a, sqrt_b, "math generations must not share sqrt");
        assert_ne!(sqrt_a.value_id(), sqrt_b.value_id());
        let (ValueKind::BuiltinFunction(name_a), ValueKind::BuiltinFunction(name_b)) =
            (sqrt_a.kind(), sqrt_b.kind())
        else {
            panic!("math.sqrt must be a BuiltinFunction");
        };
        assert!(
            std::ptr::eq(name_a, name_b),
            "fresh objects must retain the interned registry dispatch name"
        );

        let itertools_a =
            crate::builtin_modules::load_builtin_module("itertools").expect("first itertools");
        let itertools_b =
            crate::builtin_modules::load_builtin_module("itertools").expect("second itertools");
        let chain_a = module_attr(&itertools_a, "chain");
        let chain_b = module_attr(&itertools_b, "chain");
        let descriptor = |class: &Value| {
            let ValueKind::PyClass(class) = class.kind() else {
                panic!("itertools.chain must be a PyClass");
            };
            class
                .borrow()
                .attrs
                .get("__next__")
                .expect("chain.__next__ descriptor")
                .clone()
        };
        let next_a = descriptor(&chain_a);
        let next_b = descriptor(&chain_b);
        assert_ne!(
            next_a, next_b,
            "native class descriptors must be generation-local"
        );
        assert_ne!(next_a.value_id(), next_b.value_id());

        let builtins_a =
            crate::builtin_modules::load_builtin_module("builtins").expect("first builtins");
        let builtins_b =
            crate::builtin_modules::load_builtin_module("builtins").expect("second builtins");
        let len_a = module_attr(&builtins_a, "len");
        let len_b = module_attr(&builtins_b, "len");
        assert_eq!(len_a, len_b, "flat builtins must retain shared identity");
        assert_eq!(len_a.value_id(), len_b.value_id());
    }

    #[test]
    fn duplicate_registration_panics_at_init() {
        // Tier 1 and the module table are sorted and checked while the crate
        // compiles, so a duplicate there never reaches a test.  Each module's
        // own slice is checked when the module is first used; registering
        // every module here runs that check for all of them.
        assert!(lookup("abs").is_some());
        for module in &MODULES {
            assert!(module_regs(module.name).is_some());
        }
        assert_eq!(
            registered_modules().len(),
            MODULES.len(),
            "every module must have passed its registration check"
        );
    }

    #[test]
    #[should_panic(expected = "duplicate or unsorted built-in name in module `m`")]
    fn module_registration_rejects_a_duplicate_name() {
        static REGS: &[BuiltinReg] = &[test_reg("m.a"), test_reg("m.b"), test_reg("m.b")];
        validate_module(&ModuleRegs {
            name: "m",
            regs: REGS,
        });
    }

    #[test]
    #[should_panic(expected = "duplicate or unsorted built-in name in module `m`")]
    fn module_registration_rejects_an_unsorted_slice() {
        static REGS: &[BuiltinReg] = &[test_reg("m.b"), test_reg("m.a")];
        validate_module(&ModuleRegs {
            name: "m",
            regs: REGS,
        });
    }

    #[test]
    #[should_panic(expected = "is not named `m.<name>`")]
    fn module_registration_rejects_a_name_outside_the_module_prefix() {
        // Such a name could never be resolved: lookups find a module by the
        // dotted prefix of the name they are given.
        static REGS: &[BuiltinReg] = &[test_reg("mx.a")];
        validate_module(&ModuleRegs {
            name: "m",
            regs: REGS,
        });
    }

    #[test]
    fn compile_time_tables_are_sorted_from_any_declaration_order() {
        static FIRST: &[BuiltinReg] = &[test_reg("zeta"), test_reg("alpha.b")];
        static SECOND: &[BuiltinReg] = &[test_reg("mid"), test_reg("alpha")];
        static EXTRA: &[BuiltinReg] = &[test_reg("beta")];
        const PARTS: &[&[BuiltinReg]] = &[FIRST, &[], SECOND];
        const SORTED: [BuiltinReg; 5] = sorted_regs(PARTS, EXTRA);
        assert_eq!(count_regs(PARTS, EXTRA), 5);
        let names: Vec<&str> = SORTED
            .iter()
            .map(|registration| registration.name)
            .collect();
        assert_eq!(names, ["alpha", "alpha.b", "beta", "mid", "zeta"]);

        // Modules without native callables get no row.
        static TABLE: &[ModuleRegs] = &[
            ModuleRegs {
                name: "os",
                regs: FIRST,
            },
            ModuleRegs {
                name: "json",
                regs: &[],
            },
            ModuleRegs {
                name: "math",
                regs: SECOND,
            },
        ];
        const TABLES: &[&[ModuleRegs]] = &[TABLE];
        const MODULE_ROWS: [ModuleRegs; 2] = sorted_modules(TABLES, &[]);
        assert_eq!(count_modules(TABLES, &[]), 2);
        let names: Vec<&str> = MODULE_ROWS.iter().map(|module| module.name).collect();
        assert_eq!(names, ["math", "os"]);
    }

    #[test]
    #[should_panic(expected = "twice")]
    fn compile_time_tables_reject_a_duplicate_name() {
        // Evaluated at run time here; in a `static` initialiser the same
        // panic stops the build and names the duplicate.
        static FIRST: &[BuiltinReg] = &[test_reg("once"), test_reg("twice")];
        static SECOND: &[BuiltinReg] = &[test_reg("twice")];
        let _ = sorted_regs::<3>(&[FIRST, SECOND], &[]);
    }

    #[test]
    fn lookup_name_returns_interned_static_str() {
        // `lookup_name` must return the exact `&'static str` pointer stored
        // in the registry, not a freshly allocated copy.  This pins the
        // contract that `Value::builtin_function(lookup_name(n).unwrap())`
        // reuses the single interned string rather than leaking a new Box.
        let name = lookup_name("abs").expect("abs must be registered");
        assert_eq!(name, "abs");
        // The returned pointer must be the one stored in the registry entry.
        let reg_name = TIER1
            .iter()
            .find(|r| r.name == "abs")
            .expect("abs in registry")
            .name;
        assert!(
            std::ptr::eq(name, reg_name),
            "lookup_name must return the registry's own &'static str pointer"
        );
    }

    #[test]
    fn every_flat_namespace_registry_entry_has_a_name() {
        // Acceptance criterion from issue #440: every flat-namespace builtin
        // registered in the registry must be reachable via `lookup_name`.
        // Since `resolve_builtin` in `expr.rs` delegates the non-primitive,
        // non-NotImplemented path entirely to `lookup_name`, this test
        // guarantees that a new `fn foo(…)` added to a `@flat pyrust_module!`
        // body becomes accessible as a bare global with no edits to `expr.rs`.
        for reg in crate::builtin_modules::all_regs() {
            // Module-namespaced names (`math.sqrt`, …) are not bare-global
            // candidates; skip them.
            if reg.name.contains('.') {
                continue;
            }
            let found = lookup_name(reg.name);
            assert!(
                found.is_some(),
                "registered flat builtin {:?} must be found by lookup_name",
                reg.name
            );
            assert_eq!(
                found.unwrap(),
                reg.name,
                "lookup_name must return the same string it was queried with"
            );
        }
    }

    // ── Lazy per-module registration (issue #3291) ───────────────────────────

    /// Modules a start-up must never register: none of them is imported by
    /// a bare interpreter.
    const UNIMPORTED_STDLIB: [&str; 10] = [
        "asyncio", "decimal", "email", "http", "json", "math", "sqlite3", "ssl", "xml", "zlib",
    ];

    #[test]
    fn tier_one_is_exactly_builtins_and_structseq() {
        let expected = crate::builtin_modules::builtins::REG_FAMILIES
            .iter()
            .map(|family| family.len())
            .sum::<usize>()
            + crate::builtin_modules::macros::STRUCTSEQ_REGS.len();
        assert_eq!(TIER1.len(), expected);
        assert_eq!(stats().tier1_callables, expected);
        for registration in &TIER1 {
            assert!(
                registration.metadata.declaring_module == "builtins",
                "{:?} is declared by `{}`; only `builtins` belongs in tier 1",
                registration.name,
                registration.metadata.declaring_module
            );
        }
        // The flat module's callables are tier 1, so it has no tier-2 row.
        assert!(MODULES.iter().all(|module| module.name != "builtins"));
    }

    #[test]
    fn bare_interpreter_startup_consults_no_lazy_module() {
        // The in-process equivalent of `pyrust -S -c "pass"`: construct the
        // interpreter, run CLI start-up without `site`, execute `pass`, and
        // tear the interpreter down.
        let ((), consulted) = consulted_during(|| {
            let mut interp = Interpreter::with_command_args(&[]);
            interp.set_runtime_config(crate::interpreter::RuntimeConfig {
                no_site: 1,
                ..Default::default()
            });
            interp.initialize_cli_startup().expect("CLI start-up");
            interp.exec_source("pass", None, None).expect("pass");
        });
        assert!(
            consulted.is_empty(),
            "start-up must resolve every callable from tier 1, but consulted {consulted:?}"
        );
        for module in UNIMPORTED_STDLIB {
            assert!(
                !consulted.contains(&module),
                "start-up consulted un-imported module {module}"
            );
        }
    }

    #[test]
    fn importing_a_module_registers_it_only_when_a_callable_is_used() {
        let mut interp = Interpreter::default();
        let globals = Value::dict(crate::value::PyDict::default());
        let mut run = |source: &str| {
            let globals = globals.clone();
            consulted_during(|| {
                interp
                    .exec_source(source, Some(globals), None)
                    .unwrap_or_else(|e| panic!("{source:?} failed: {e:?}"))
            })
            .1
        };

        // Building the module binds its functions by name; it does not read
        // the module's registrations.
        assert_eq!(run("import math, itertools"), [] as [&str; 0]);

        // A module function: only `math` is registered, and the call works.
        assert_eq!(run("assert math.sqrt(9.0) == 3.0"), ["math"]);
        assert_eq!(
            run("assert math.floor(2.5) == 2 and math.gcd(12, 18) == 6"),
            ["math"]
        );

        // A native class: construction, a method, and iteration all resolve
        // through `itertools` alone.
        assert_eq!(
            run("c = itertools.chain([1], [2])\nassert next(c) == 1 and list(c) == [2]"),
            ["itertools"]
        );

        // Introspection reads the same registration the call does.
        assert_eq!(
            run("assert math.sqrt.__name__ == 'sqrt'\n\
                 assert math.sqrt.__qualname__ == 'sqrt'\n\
                 assert math.sqrt.__module__ == 'math'\n\
                 assert repr(math.sqrt) == '<built-in function sqrt>'"),
            ["math"]
        );
    }

    #[test]
    fn cold_qualified_lookup_consults_only_the_owning_module() {
        // A module function: one module consulted, found on the first prefix.
        let (found, consulted) = consulted_during(|| lookup_registration("math.sqrt"));
        assert_eq!(
            found.map(|registration| registration.name),
            Some("math.sqrt")
        );
        assert_eq!(consulted, ["math"]);

        // A dotted module: the longest prefix owns the name, so the parent
        // package is never consulted.
        let nested = module_row("os.path").regs[0].name;
        let (found, consulted) = consulted_during(|| lookup_registration(nested));
        assert!(found.is_some_and(|registration| std::ptr::eq(registration.name, nested)));
        assert_eq!(consulted, ["os.path"]);

        // A class member: `os.stat_result` is not a module, so the search
        // falls back to the shorter prefix that is.
        let (found, consulted) =
            consulted_during(|| lookup_registration("os.stat_result.__repr__"));
        assert!(found.is_some());
        assert_eq!(consulted, ["os"]);

        // A generated codec submodule and an internal shim resolve the same
        // way as a module declared in `pyrust_builtin_modules!`.
        for module in ["encodings.utf_8", "_thread", "asyncio.format_helpers"] {
            let name = module_row(module).regs[0].name;
            let (found, consulted) = consulted_during(|| lookup_registration(name));
            assert!(found.is_some(), "{name} must resolve");
            assert_eq!(consulted, [module]);
        }

        // A miss inside a real module consults that module and nothing else.
        let (found, consulted) = consulted_during(|| lookup_registration("math.no_such_function"));
        assert!(found.is_none());
        assert_eq!(consulted, ["math"]);

        // Names no module owns consult nothing: tier-1 hits, undotted
        // misses, and dotted names whose prefix is a type rather than a
        // module.
        for name in [
            "len",
            "list.__getitem__",
            "_structseq.__new__",
            "absolutely-not-a-builtin",
            "str.no_such_method",
            "builtins.abs",
        ] {
            let (_, consulted) = consulted_during(|| lookup_registration(name));
            assert_eq!(consulted, [] as [&str; 0], "{name:?} consulted a module");
        }
    }

    #[test]
    fn memo_answers_repeat_lookups_only_for_the_registrys_own_name() {
        // Private slots: the shared memo is written by every test thread.
        let registration = lookup_registration("math.sqrt").expect("math.sqrt");
        let own_name = registration.name;

        // Searching with the registry's own string remembers the location,
        // and the remembered word resolves to the very same registration.
        let slot = AtomicU32::new(0);
        let found = lookup_uncached(own_name, &slot).expect("math.sqrt by its own name");
        assert!(std::ptr::eq(found, registration));
        let word = slot.load(Ordering::Relaxed);
        assert_ne!(word, 0, "the lookup must be remembered");
        let recalled = recall(word, own_name).expect("memo hit");
        assert!(std::ptr::eq(recalled, registration));

        // An equal string at another address is found by searching and is
        // not remembered: its address says nothing about its contents.
        let copy = String::from("math.sqrt");
        let slot = AtomicU32::new(0);
        let found = lookup_uncached(&copy, &slot).expect("math.sqrt by an equal string");
        assert!(std::ptr::eq(found, registration));
        assert_eq!(slot.load(Ordering::Relaxed), 0);
        assert!(recall(word, &copy).is_none());

        // A prefix of a registered name shares its address but not its
        // length, so it is not mistaken for that name.
        let prefix = &own_name[..own_name.len() - 1];
        assert!(std::ptr::eq(prefix.as_ptr(), own_name.as_ptr()));
        assert!(recall(word, prefix).is_none());
        assert!(lookup_registration(prefix).is_none());

        // Empty, out-of-range, and foreign words are never trusted.
        let abs_word = locate("abs")
            .and_then(Location::pack)
            .expect("abs location");
        for untrusted in [0, u32::MAX, abs_word] {
            assert!(recall(untrusted, own_name).is_none());
        }
        assert!(recall(abs_word, lookup_name("abs").expect("abs")).is_some());

        // The public lookup agrees with the search whatever the shared memo
        // currently holds.
        for _ in 0..3 {
            assert!(std::ptr::eq(
                lookup_registration(own_name).expect("math.sqrt"),
                registration
            ));
        }
    }

    #[test]
    fn every_location_fits_in_a_memo_word() {
        // A location that did not fit would still resolve, just never from
        // the memo; keep the tables within the packed range so none is left
        // out.
        assert!(MODULES.len() < usize::from(u16::MAX));
        assert!(TIER1.len() <= usize::from(u16::MAX));
        for module in &MODULES {
            assert!(
                module.regs.len() <= usize::from(u16::MAX),
                "`{}` has too many registrations to memoise",
                module.name
            );
        }
        for location in [
            Location { table: 0, index: 0 },
            Location {
                table: MODULES.len(),
                index: usize::from(u16::MAX) - 1,
            },
        ] {
            let unpacked = location
                .pack()
                .and_then(Location::unpack)
                .expect("location round-trips");
            assert_eq!(
                (unpacked.table, unpacked.index),
                (location.table, location.index)
            );
        }
    }

    #[test]
    fn module_regs_finds_exactly_the_modules_with_native_callables() {
        let math = module_regs("math").expect("math has native callables");
        assert!(
            math.iter()
                .any(|registration| registration.name == "math.sqrt")
        );
        assert!(std::ptr::eq(math, module_row("math").regs));

        // Not a module, a pure-Python module, and the flat `builtins`
        // module (tier 1) have no registrations to resolve.
        for name in ["not_a_module", "dataclasses", "builtins", "math.sqrt", ""] {
            assert!(module_regs(name).is_none(), "{name:?} must have no row");
        }
    }

    #[test]
    fn registration_sources_build_nothing_at_run_time() {
        // Names and `REGS` slices are constants: the derive crate and the
        // module definitions must not leak strings or lazily build vectors.
        // The needles are assembled here so this file never matches itself.
        let forbidden = [
            ["Box", "::leak"].concat(),
            ["LazyLock<", "Vec<"].concat(),
            ["LazyLock<&'static ", "str>"].concat(),
            ["REGS", "_CELL"].concat(),
        ];
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut pending = vec![
            manifest.join("../pyrust-derive/src"),
            manifest.join("src/builtin_modules"),
        ];
        let mut scanned = 0usize;
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{dir:?}: {e}")) {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("read source");
                scanned += 1;
                for needle in &forbidden {
                    assert!(
                        !source.contains(needle.as_str()),
                        "{} contains `{needle}`",
                        path.display()
                    );
                }
            }
        }
        assert!(scanned > 300, "scanned only {scanned} source files");
    }
}
