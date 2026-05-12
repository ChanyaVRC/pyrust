//! Per-module registration slices for built-in callables.
//!
//! Each module file annotates functions with `#[pyfunction(name = "...")]`
//! (from `pyrust-derive`).  The macro emits a sibling registration constant
//! per annotated function; this `mod.rs` exposes the per-module slice via
//! a `REGS: &[BuiltinReg]` constant that the central registry concatenates.

pub mod math;
pub mod sys;
