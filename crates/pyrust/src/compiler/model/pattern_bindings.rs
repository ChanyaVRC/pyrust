/// Collect all names that a pattern binds on a successful match.
/// Used by `Pattern::Or` validation to enforce that every alternative binds
/// the same set of names (PEP 634 / CPython 3.12 `SyntaxError`).
fn pattern_bound_names(pat: &Pattern) -> HashSet<String> {
    match pat {
        Pattern::Capture(name) => {
            let mut s = HashSet::new();
            s.insert(name.clone());
            s
        }
        Pattern::As { pattern, name } => {
            let mut s = pattern_bound_names(pattern);
            s.insert(name.clone());
            s
        }
        Pattern::Sequence(elements) => {
            let mut s = HashSet::new();
            for (elem_pat, _is_star) in elements {
                s.extend(pattern_bound_names(elem_pat));
            }
            s
        }
        Pattern::Mapping(pairs, rest_name) => {
            let mut s = HashSet::new();
            for (_key, val_pat) in pairs {
                s.extend(pattern_bound_names(val_pat));
            }
            if let Some(rest) = rest_name {
                s.insert(rest.clone());
            }
            s
        }
        Pattern::Class {
            positional, kwargs, ..
        } => {
            let mut s = HashSet::new();
            for pat in positional {
                s.extend(pattern_bound_names(pat));
            }
            for (_attr, pat) in kwargs {
                s.extend(pattern_bound_names(pat));
            }
            s
        }
        Pattern::Or(alternatives) => {
            // All alternatives must bind the same names; return the first's set.
            alternatives
                .first()
                .map(pattern_bound_names)
                .unwrap_or_default()
        }
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Value(_) => HashSet::new(),
    }
}

/// Walk the leading edge of a pattern — descending into the first alternative
/// of nested `Pattern::Or` nodes — and return the name of the first bare
/// `Pattern::Capture` found, or `None` if the leading edge is not a capture.
///
/// Used by the `Pattern::Or` unreachable-check to detect cases like
/// `case (x | 1) | z:` where the inner OR's first alternative `x` is a
/// capture that makes subsequent outer alternatives unreachable.
fn or_leading_capture(pat: &Pattern) -> Option<&str> {
    match pat {
        Pattern::Capture(name) if name != "_" => Some(name),
        Pattern::Or(alts) => alts.first().and_then(or_leading_capture),
        _ => None,
    }
}

/// Same as `or_leading_capture` but for wildcards.
fn or_leading_is_wildcard(pat: &Pattern) -> bool {
    match pat {
        Pattern::Wildcard => true,
        Pattern::Or(alts) => alts.first().is_some_and(or_leading_is_wildcard),
        _ => false,
    }
}
