#[cfg(test)]
mod module_mutation_state_tests {
    use super::ModuleMutationState;

    #[test]
    fn saturated_module_version_never_wraps_or_matches_a_cache() {
        let state = ModuleMutationState::fresh();
        state.0.set(u64::MAX - 1);
        state.bump();
        assert_eq!(state.version(), u64::MAX);
        state.bump();
        assert_eq!(state.version(), u64::MAX);
        assert_eq!(state.cache_version(), None);
        assert!(!state.matches_cache_version(u64::MAX));
    }
}
