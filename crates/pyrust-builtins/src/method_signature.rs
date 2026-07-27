//! Shared positional-argument policy for builtin method tables.
//!
//! Concrete method names stay in their owning type modules.  This module only
//! represents a signature and formats the common "too many positional
//! arguments" diagnostics.

use pyrust_core::{PyError, Result, Value, ValueKind};

/// Whether a known builtin method accepts any keyword arguments.
///
/// The concrete type module still owns the mapping from method name to this
/// policy.  This shared value only provides the common rejection diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeywordPolicy {
    Reject,
    Accept,
}

impl KeywordPolicy {
    /// Return whether the keyword shape is accepted without constructing a
    /// diagnostic. Builtin method specs use this before resolving their
    /// concrete method name, keeping the normal no-keyword path name-free.
    #[inline(always)]
    pub const fn accepts(self, has_keywords: bool) -> bool {
        !has_keywords || matches!(self, Self::Accept)
    }

    /// Reject a non-empty keyword map before positional validation or any
    /// argument conversion can run.
    #[inline]
    pub fn validate(self, owner: &str, method: &str, has_keywords: bool) -> Result<()> {
        if self.accepts(has_keywords) {
            return Ok(());
        }
        reject_keywords(owner, method)
    }
}

#[cold]
#[inline(never)]
fn reject_keywords(owner: &str, method: &str) -> Result<()> {
    Err(PyError::named(
        "TypeError",
        format!("{owner}.{method}() takes no keyword arguments"),
    ))
}

/// Read an optional boolean that an interpreter-aware adapter has already
/// normalised through Python's truth protocol.
///
/// Interpreter-free builtin implementations must not approximate
/// `__bool__`/`__len__` dispatch with `Value::truthy_raw()`.  Keeping this
/// guard at the core boundary makes an accidentally unnormalised call fail
/// loudly instead of silently changing Python semantics.
#[inline(always)]
pub(crate) fn normalized_optional_bool(
    owner: &str,
    method: &str,
    parameter: &str,
    args: &[Value],
) -> Result<bool> {
    match args {
        [] => Ok(false),
        [value] => match value.kind() {
            ValueKind::Bool(value) => Ok(value),
            _ => normalized_bool_precondition_error(owner, method, parameter),
        },
        _ => normalized_bool_precondition_error(owner, method, parameter),
    }
}

#[cold]
#[inline(never)]
fn normalized_bool_precondition_error(owner: &str, method: &str, parameter: &str) -> Result<bool> {
    Err(PyError::named(
        "TypeError",
        format!(
            "internal precondition violated: {owner}.{method}() {parameter} must be normalized to bool"
        ),
    ))
}

/// Positional arity accepted by a builtin method, excluding its receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PositionalArity {
    min: u8,
    max: Option<u8>,
    overflow_style: OverflowStyle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverflowStyle {
    Expected,
    TakesAtMost,
    NoPositional,
}

impl PositionalArity {
    #[inline(always)]
    pub const fn exact(count: u8) -> Self {
        Self {
            min: count,
            max: Some(count),
            overflow_style: OverflowStyle::Expected,
        }
    }

    #[inline(always)]
    pub const fn range(min: u8, max: u8) -> Self {
        Self {
            min,
            max: Some(max),
            overflow_style: OverflowStyle::Expected,
        }
    }

    /// Range whose established CPython/PyRust diagnostic uses
    /// `method() takes at most N arguments (M given)`.
    #[inline(always)]
    pub const fn range_takes_at_most(min: u8, max: u8) -> Self {
        Self {
            min,
            max: Some(max),
            overflow_style: OverflowStyle::TakesAtMost,
        }
    }

    #[inline(always)]
    pub const fn no_positional() -> Self {
        Self {
            min: 0,
            max: Some(0),
            overflow_style: OverflowStyle::NoPositional,
        }
    }

    #[inline(always)]
    pub const fn variadic(min: u8) -> Self {
        Self {
            min,
            max: None,
            overflow_style: OverflowStyle::Expected,
        }
    }

    #[inline(always)]
    pub const fn min(self) -> usize {
        self.min as usize
    }

    #[inline(always)]
    pub const fn max(self) -> Option<usize> {
        match self.max {
            Some(max) => Some(max as usize),
            None => None,
        }
    }

    /// Return whether `given` does not exceed this signature's positional
    /// maximum. Underflow remains intentionally accepted here and is diagnosed
    /// by the concrete method body.
    #[inline(always)]
    pub const fn accepts(self, given: usize) -> bool {
        match self.max {
            Some(max) => given <= max as usize,
            None => true,
        }
    }

    /// Reject excess operands before a method body can inspect or convert any
    /// argument.
    ///
    /// Underflow remains with the existing method bodies for now.  Those
    /// implementations intentionally carry CPython-compatible, method-specific
    /// diagnostics; moving underflow here would change established messages.
    #[inline]
    pub fn reject_excess(self, owner: &str, method: &str, given: usize) -> Result<()> {
        if self.accepts(given) {
            return Ok(());
        }
        reject_excess_cold(self, owner, method, given)
    }
}

#[cold]
#[inline(never)]
fn reject_excess_cold(
    arity: PositionalArity,
    owner: &str,
    method: &str,
    given: usize,
) -> Result<()> {
    let max = arity
        .max()
        .expect("reject_excess_cold requires a bounded signature");
    let message = if arity.overflow_style == OverflowStyle::NoPositional {
        format!("{method}() takes no positional arguments")
    } else if arity.overflow_style == OverflowStyle::TakesAtMost {
        let noun = argument_noun(max);
        format!("{method}() takes at most {max} {noun} ({given} given)")
    } else if max == 0 {
        format!("{owner}.{method}() takes no arguments ({given} given)")
    } else if arity.min() == 1 && max == 1 {
        format!("{owner}.{method}() takes exactly one argument ({given} given)")
    } else if arity.min() == max {
        format!("{method} expected {max} arguments, got {given}")
    } else {
        format!(
            "{method} expected at most {max} {}, got {given}",
            argument_noun(max)
        )
    };
    Err(PyError::named("TypeError", message))
}

const fn argument_noun(count: usize) -> &'static str {
    if count == 1 { "argument" } else { "arguments" }
}

#[cfg(test)]
mod tests {
    use super::{KeywordPolicy, PositionalArity};

    #[test]
    fn keyword_policy_short_circuits_empty_maps_and_rejects_before_bodies() {
        assert!(KeywordPolicy::Reject.accepts(false));
        assert!(!KeywordPolicy::Reject.accepts(true));
        assert!(KeywordPolicy::Accept.accepts(true));
        assert!(
            KeywordPolicy::Reject
                .validate("list", "append", false)
                .is_ok()
        );
        assert!(KeywordPolicy::Accept.validate("list", "sort", true).is_ok());
        let err = KeywordPolicy::Reject
            .validate("slice", "indices", true)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "TypeError: slice.indices() takes no keyword arguments"
        );
    }

    #[test]
    fn rejects_only_excess_arguments() {
        let exact = PositionalArity::exact(1);
        assert!(exact.accepts(0));
        assert!(exact.accepts(1));
        assert!(!exact.accepts(2));
        assert!(exact.reject_excess("list", "append", 0).is_ok());
        assert!(exact.reject_excess("list", "append", 1).is_ok());
        assert!(exact.reject_excess("list", "append", 2).is_err());

        let variadic = PositionalArity::variadic(0);
        assert!(variadic.accepts(usize::MAX));
        assert!(variadic.reject_excess("set", "union", usize::MAX).is_ok());
    }

    #[test]
    fn audited_container_method_tables_have_signatures() {
        fn assert_covered(
            owner: &str,
            methods: &[&str],
            lookup: fn(&str) -> Option<PositionalArity>,
        ) {
            for method in methods {
                assert!(
                    lookup(method).is_some(),
                    "{owner}.{method} is missing positional-arity policy"
                );
            }
        }

        assert_covered("list", crate::list::METHODS, crate::list::positional_arity);
        assert_covered(
            "tuple",
            crate::tuple::METHODS,
            crate::tuple::positional_arity,
        );
        assert_covered("dict", crate::dict::METHODS, crate::dict::positional_arity);
        assert_covered("set", crate::set::METHODS, crate::set::positional_arity);
        assert_covered(
            "frozenset",
            crate::frozenset::METHODS,
            crate::frozenset::positional_arity,
        );
        assert_covered(
            "str",
            crate::string::METHODS,
            crate::string::positional_arity,
        );
        assert_covered(
            "bytes",
            crate::bytes::METHODS,
            crate::bytes::positional_arity,
        );
        assert_covered(
            "bytearray",
            crate::bytearray::METHODS,
            crate::bytearray::positional_arity,
        );
        assert_covered(
            "slice",
            crate::slice::METHODS,
            crate::slice::positional_arity,
        );
    }

    #[test]
    fn audited_container_method_tables_have_keyword_policies() {
        fn assert_covered(
            owner: &str,
            methods: &[&str],
            lookup: fn(&str) -> Option<KeywordPolicy>,
        ) {
            for method in methods {
                assert!(
                    lookup(method).is_some(),
                    "{owner}.{method} is missing keyword policy"
                );
            }
        }

        assert_covered("list", crate::list::METHODS, crate::list::keyword_policy);
        assert_covered("tuple", crate::tuple::METHODS, crate::tuple::keyword_policy);
        assert_covered("dict", crate::dict::METHODS, crate::dict::keyword_policy);
        assert_covered("set", crate::set::METHODS, crate::set::keyword_policy);
        assert_covered(
            "frozenset",
            crate::frozenset::METHODS,
            crate::frozenset::keyword_policy,
        );
        assert_covered("str", crate::string::METHODS, crate::string::keyword_policy);
        assert_covered("bytes", crate::bytes::METHODS, crate::bytes::keyword_policy);
        assert_covered(
            "bytearray",
            crate::bytearray::METHODS,
            crate::bytearray::keyword_policy,
        );
        assert_covered("slice", crate::slice::METHODS, crate::slice::keyword_policy);

        assert!(crate::string::keyword_policy("maketrans").is_some());
        for method in ["__contains__", "fromkeys"] {
            assert!(crate::dict::keyword_policy(method).is_some());
        }
        assert!(crate::set::keyword_policy("__contains__").is_some());
        for method in ["fromhex", "maketrans"] {
            assert!(crate::bytes::keyword_policy(method).is_some());
            assert!(crate::bytearray::keyword_policy(method).is_some());
        }
    }

    #[test]
    fn variadic_families_remain_unbounded() {
        for method in [
            "update",
            "intersection_update",
            "difference_update",
            "union",
            "intersection",
            "difference",
        ] {
            assert_eq!(crate::set::positional_arity(method).unwrap().max(), None);
        }
        for method in ["union", "intersection", "difference"] {
            assert_eq!(
                crate::frozenset::positional_arity(method).unwrap().max(),
                None
            );
        }
        assert_eq!(
            crate::string::positional_arity("format").unwrap().max(),
            None
        );
    }
}
