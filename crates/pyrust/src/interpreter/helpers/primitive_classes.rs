// Primitive-class helpers intentionally share the interpreter helper scope.
// The facade keeps existing visibility while each fragment owns one concern.
include!("primitive_classes/model.rs");
include!("primitive_classes/bootstrap.rs");
include!("primitive_classes/metaclasses.rs");
include!("primitive_classes/lookup.rs");
include!("primitive_classes/construction.rs");

#[cfg(test)]
include!("primitive_classes/tests.rs");
