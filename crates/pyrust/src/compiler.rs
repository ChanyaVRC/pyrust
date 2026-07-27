// Compiler implementation is split by compilation responsibility. Keeping the
// includes in this file preserves the original module's private-item visibility.

include!("compiler/api.rs");
include!("compiler/free_var_reads.rs");
include!("compiler/cell_analysis.rs");
include!("compiler/binding_analysis.rs");
include!("compiler/comprehension_analysis.rs");
include!("compiler/class_scope_analysis.rs");
include!("compiler/free_variables.rs");
include!("compiler/ast_rewrites.rs");
include!("compiler/model.rs");
include!("compiler/core.rs");
include!("compiler/statements.rs");
include!("compiler/patterns.rs");
include!("compiler/loops.rs");
include!("compiler/raise_delete_import.rs");
include!("compiler/functions.rs");
include!("compiler/classes.rs");
include!("compiler/try_with.rs");
include!("compiler/expressions.rs");
include!("compiler/comprehensions.rs");
include!("compiler/calls.rs");
include!("compiler/literals.rs");

#[cfg(test)]
#[path = "compiler/tests.rs"]
mod tests;
