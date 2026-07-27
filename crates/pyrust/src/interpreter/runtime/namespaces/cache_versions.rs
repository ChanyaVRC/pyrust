/// Bump the global-namespace *structure* version (invalidating every cached
/// built-in resolution) AND the value version. Used for `del`, the cold assign
/// paths, and built-in-shadowing writes: any change that can make a name
/// resolve to a different built-in-vs-global than before.
#[inline]
pub(crate) fn bump_global_struct_version(interp: &Interpreter) {
    let environment = interp.env.borrow();
    environment.bump_namespace_structure_version();
    environment.bump_filesystem_module_mutation();
}
