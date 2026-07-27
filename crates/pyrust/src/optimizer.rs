#![allow(clippy::needless_range_loop)]
// The optimizer is organized by pass family; all includes expand in this module
// so passes retain the same private visibility and can share analysis helpers.

include!("optimizer/effects.rs");
include!("optimizer/pipeline.rs");
include!("optimizer/constant_folding.rs");
include!("optimizer/register_analysis.rs");
include!("optimizer/loop_motion.rs");
include!("optimizer/elimination.rs");
include!("optimizer/propagation.rs");
include!("optimizer/control_flow.rs");

#[cfg(test)]
#[path = "optimizer/tests/mod.rs"]
mod tests;
