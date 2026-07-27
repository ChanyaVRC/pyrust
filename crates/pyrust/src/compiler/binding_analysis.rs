// Shared analysis helpers for names bound by assignment targets.

/// Add every plain name bound by `target` to `names`.
///
/// Attribute, item, and slice targets mutate an object rather than binding a
/// name in the current scope, so they contribute nothing.
fn collect_written_target(target: &AssignTarget, names: &mut HashSet<String>) {
    match target {
        AssignTarget::Name(name) => {
            names.insert(name.clone());
        }
        AssignTarget::Tuple(targets) => {
            for target in targets {
                collect_written_target(target, names);
            }
        }
        AssignTarget::Starred(inner) => collect_written_target(inner, names),
        AssignTarget::Attr(..) | AssignTarget::Index(..) | AssignTarget::Slice { .. } => {}
    }
}
