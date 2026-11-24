//! Bytecode-owned inline-cache protocols and their explicit public surface.

mod attr;
mod binop;
mod global;
mod kw_call;
mod storage;

pub use attr::AttrCacheEntry;
pub use attr::{NativeClassMethodCacheEntry, SlotAttrCacheEntry};
pub(crate) use binop::{BINOP_SPEC_THRESHOLD, BinOpCacheEntry, BinopTypeTag};
pub(crate) use global::{GlobalCacheEntry, global_cache_interest_mask};
pub use kw_call::KwCallCacheEntry;
pub use kw_call::{KwCallExArgsCache, KwCallExSimpleCache, KwCallSimpleCache};
pub(crate) use storage::{
    SiteCache, SparsePcCache, is_attr_cache_site, is_call_builtin_cache_site,
};

#[cfg(test)]
mod tests;
