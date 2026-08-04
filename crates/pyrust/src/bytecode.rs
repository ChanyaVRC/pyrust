//! Bytecode data-model facade.
//!
//! Ownership follows independent reasons to change: `function` owns function
//! prototypes and parameter plans, `instruction` owns register operands and
//! opcodes, `cache` owns bytecode-local specialization state, and `code`
//! owns compiled code objects plus source/traceback provenance. Cross-boundary
//! API remains explicit here so callers keep the existing `crate::bytecode`
//! paths without flattening the child modules internally.

mod cache;
mod code;
mod function;
mod instruction;

pub use cache::{AttrCacheEntry, KwCallCacheEntry};
pub(crate) use cache::{
    BINOP_SPEC_THRESHOLD, BinOpCacheEntry, BinopTypeTag, GlobalCacheEntry,
    global_cache_interest_mask,
};
pub use code::FnCode;
pub(crate) use code::{EXC_NO_HANDLER, MAX_FRAME_REGS};
pub use function::{CellVar, FnParamSpec, FnProto, compute_param_binds, compute_self_bind};
pub use instruction::{
    DictKeyKindHint, Insn, IntRangeExactGuard, KwCallName, NO_CLASS_LOCAL, NO_KWARGS, Reg,
};

#[cfg(test)]
mod ownership_tests {
    const CHILD_MODULES: &[(&str, &str, &str)] = &[
        (
            "function",
            include_str!("bytecode/function.rs"),
            "pub struct FnProto",
        ),
        (
            "instruction",
            include_str!("bytecode/instruction.rs"),
            "pub enum Insn",
        ),
        (
            "code",
            include_str!("bytecode/code.rs"),
            "pub struct FnCode",
        ),
    ];

    const CACHE_FACADE: &str = include_str!("bytecode/cache.rs");
    const CACHE_CHILD_MODULES: &[(&str, &str, &str)] = &[
        (
            "attr",
            include_str!("bytecode/cache/attr.rs"),
            "pub enum AttrCacheEntry",
        ),
        (
            "kw_call",
            include_str!("bytecode/cache/kw_call.rs"),
            "pub enum KwCallCacheEntry",
        ),
        (
            "binop",
            include_str!("bytecode/cache/binop.rs"),
            "pub(crate) enum BinOpCacheEntry",
        ),
        (
            "global",
            include_str!("bytecode/cache/global.rs"),
            "pub(crate) enum GlobalCacheEntry",
        ),
        (
            "tests",
            include_str!("bytecode/cache/tests.rs"),
            "fn global_cache_entries_guard_namespace_and_shared_module_mutation",
        ),
    ];

    #[test]
    fn child_modules_keep_explicit_ownership_boundaries() {
        for (module, source, owned_item) in CHILD_MODULES {
            assert!(
                source.contains(owned_item),
                "bytecode::{module} must continue to own {owned_item}"
            );
            assert!(
                !source.contains("::*"),
                "bytecode::{module} must use explicit imports and re-exports"
            );
            assert!(
                !source.contains("include!("),
                "bytecode::{module} must remain a real Rust module, not an include fragment"
            );
        }
    }

    #[test]
    fn cache_facade_uses_real_children_and_explicit_reexports() {
        // `include_str!` embeds the checkout's line endings, so a CRLF
        // working copy (Windows CI) must not fail the multi-line pattern.
        let cache_facade = CACHE_FACADE.replace("\r\n", "\n");
        for declaration in [
            "mod attr;",
            "mod binop;",
            "mod global;",
            "mod kw_call;",
            "#[cfg(test)]\nmod tests;",
        ] {
            assert!(
                cache_facade.contains(declaration),
                "bytecode::cache facade must declare the real child module: {declaration}"
            );
        }
        for reexport in [
            "pub use attr::AttrCacheEntry;",
            "pub(crate) use binop::{BINOP_SPEC_THRESHOLD, BinOpCacheEntry, BinopTypeTag};",
            "pub(crate) use global::{GlobalCacheEntry, global_cache_interest_mask};",
            "pub use kw_call::KwCallCacheEntry;",
        ] {
            assert!(
                CACHE_FACADE.contains(reexport),
                "bytecode::cache facade must retain the explicit re-export: {reexport}"
            );
        }
        for declaration in [
            "pub enum AttrCacheEntry",
            "pub enum KwCallCacheEntry",
            "pub(crate) enum BinOpCacheEntry",
            "pub(crate) enum GlobalCacheEntry",
        ] {
            assert!(
                !CACHE_FACADE.contains(declaration),
                "cache protocol declarations belong to real child modules: {declaration}"
            );
        }
        assert!(
            !CACHE_FACADE.contains("::*"),
            "bytecode::cache facade must use explicit re-exports"
        );
        assert!(
            !CACHE_FACADE.contains("include!("),
            "bytecode::cache facade must declare real Rust child modules"
        );

        for (module, source, owned_item) in CACHE_CHILD_MODULES {
            assert!(
                source.contains(owned_item),
                "bytecode::cache::{module} must continue to own {owned_item}"
            );
            assert!(
                !source.contains("::*"),
                "bytecode::cache::{module} must use explicit imports"
            );
            assert!(
                !source.contains("include!("),
                "bytecode::cache::{module} must remain a real Rust module"
            );
        }
    }

    #[test]
    fn fn_code_keeps_one_final_resolution_cache_per_global_name() {
        let source = include_str!("bytecode/code.rs");
        assert!(
            source.contains("pub(crate) global_cache: RefCell<Vec<GlobalCacheEntry>>"),
            "FnCode must retain the unified per-name LoadGlobal cache"
        );
        assert!(
            !source.contains("pub(crate) builtin_cache:"),
            "builtin fallback is a GlobalCacheEntry variant, not a parallel cache"
        );
    }
}
