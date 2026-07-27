// Builtin callable presentation is supplied by the interpreter-owned
// provider. Core owns only the neutral categories consumed by Value repr and
// type-name logic; it deliberately has no Python API-name tables or dispatch
// key parsing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinCallablePresentation<'a> {
    Function { name: &'a str },
    MethodDescriptor { owner: &'a str, name: &'a str },
    WrapperDescriptor { owner: &'a str, name: &'a str },
}

impl BuiltinCallablePresentation<'_> {
    pub const fn type_name(self) -> &'static str {
        match self {
            Self::Function { .. } => "builtin_function_or_method",
            Self::MethodDescriptor { .. } => "method_descriptor",
            Self::WrapperDescriptor { .. } => "wrapper_descriptor",
        }
    }

    pub const fn is_wrapper_descriptor(self) -> bool {
        matches!(self, Self::WrapperDescriptor { .. })
    }
}

pub type BuiltinCallablePresentationProvider =
    for<'a> fn(&'a str) -> BuiltinCallablePresentation<'a>;

static BUILTIN_CALLABLE_PRESENTATION_PROVIDER: std::sync::OnceLock<
    BuiltinCallablePresentationProvider,
> = std::sync::OnceLock::new();

/// Install the semantic owner for builtin callable presentation.
///
/// Safe to call multiple times; every interpreter instance installs the same
/// provider and only the first call wins.
pub fn install_builtin_callable_presentation_provider(
    provider: BuiltinCallablePresentationProvider,
) {
    let _ = BUILTIN_CALLABLE_PRESENTATION_PROVIDER.set(provider);
}

/// Classify a builtin dispatch token without exposing its representation to
/// core. Before an interpreter installs a provider, the conservative fallback
/// is a regular builtin function.
pub fn builtin_callable_presentation(name: &str) -> BuiltinCallablePresentation<'_> {
    BUILTIN_CALLABLE_PRESENTATION_PROVIDER
        .get()
        .map_or(BuiltinCallablePresentation::Function { name }, |provider| {
            provider(name)
        })
}
