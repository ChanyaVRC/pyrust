// Grammar implementation is grouped by syntactic responsibility. All includes
// share the parser module's private helpers and public Parser facade.

include!("parser/model.rs");
include!("parser/statements.rs");
include!("parser/patterns.rs");
include!("parser/definitions.rs");
include!("parser/control_flow.rs");
include!("parser/expressions.rs");
include!("parser/operators.rs");
include!("parser/postfix.rs");
include!("parser/primaries.rs");
include!("parser/fstrings.rs");
include!("parser/assignment.rs");
include!("parser/validation.rs");
