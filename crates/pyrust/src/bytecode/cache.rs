//! Bytecode-owned inline-cache protocols and their explicit public surface.

mod attr;
mod binop;
mod global;
mod kw_call;

pub use attr::AttrCacheEntry;
pub(crate) use binop::{BINOP_SPEC_THRESHOLD, BinOpCacheEntry, BinopTypeTag};
pub(crate) use global::{GlobalCacheEntry, global_cache_interest_mask};
pub use kw_call::KwCallCacheEntry;

#[cfg(test)]
mod tests;
