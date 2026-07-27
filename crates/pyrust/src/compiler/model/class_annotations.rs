fn class_body_has_annotations(body: &[Stmt]) -> bool {
    body.iter().any(|s| match s {
        Stmt::AnnAssign { .. } => true,
        Stmt::If {
            branches,
            else_branch,
            ..
        } => {
            branches.iter().any(|(_, b)| class_body_has_annotations(b))
                || else_branch
                    .as_deref()
                    .is_some_and(class_body_has_annotations)
        }
        Stmt::While { body, .. } | Stmt::For { body, .. } => class_body_has_annotations(body),
        _ => false,
    })
}
