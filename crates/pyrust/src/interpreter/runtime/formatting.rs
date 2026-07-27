// Formatting owns format-spec parsing, template parsing/caching, field access,
// conversion, and format-field lookup. It depends on the interpreter only when
// a Python protocol or user callable must be invoked.

include!("formatting/value_repr_support.rs");
include!("formatting/value_repr.rs");
include!("formatting/value_str.rs");
include!("formatting/spec_api.rs");
include!("formatting/spec_parser.rs");
include!("formatting/spec_renderer.rs");
include!("formatting/numeric_integer.rs");
include!("formatting/numeric_float_complex.rs");
include!("formatting/numeric_layout.rs");
include!("formatting/runtime.rs");
include!("formatting/template.rs");
include!("formatting/printf_conversion.rs");
include!("formatting/printf_string.rs");
include!("formatting/printf_bytes.rs");
include!("formatting/printf_support.rs");
