#[cfg(test)]
mod mutation_version_tests {
    use super::{
        CollectionMutationState, CollectionMutationStateInner, DictIterationLayout,
        MutableContainerKey, MutableContainerKind, MutableContainerTarget, PyDict, Rc, RefCell,
        Weak, bump_active_collection_mutation_state,
    };
    use std::cell::Cell;

    #[test]
    fn active_collection_version_wraps_but_permanently_disables_cache_matching() {
        let dict = Rc::new(RefCell::new(PyDict::default()));
        let key = MutableContainerKey {
            kind: MutableContainerKind::Dict,
            ptr: Rc::as_ptr(&dict) as usize,
        };
        let layout = DictIterationLayout::for_len(0);
        let inner = Rc::new(CollectionMutationStateInner {
            version: Cell::new(u64::MAX - 1),
            entry_order: Cell::new(0),
            cache_exhausted: Cell::new(false),
            observers: Cell::new(0),
            removal_watchers: RefCell::new(Vec::new()),
            has_watchers: Cell::new(false),
            has_mutation_activity: Cell::new(false),
            pending_removed_keys: RefCell::new(Vec::new()),
            reinserted_keys: RefCell::new(Vec::new()),
            has_dict_layout: Cell::new(true),
            dict_layout_live_len: Cell::new(layout.live_len),
            dict_layout_entry_count: Cell::new(layout.entry_count),
            dict_layout_usable: Cell::new(layout.usable),
            key,
            target: MutableContainerTarget::Dict(Weak::clone(&Rc::downgrade(&dict))),
        });
        let state = CollectionMutationState::attach(Rc::clone(&inner));
        state.0.version.set(u64::MAX - 1);
        bump_active_collection_mutation_state(Some(Rc::clone(&inner)), None);
        assert_eq!(state.version(), u64::MAX);
        bump_active_collection_mutation_state(Some(inner), None);
        assert_eq!(state.version(), 0);
        assert_eq!(state.cache_version(), None);
        assert!(!state.matches_cache_version(0));
    }

    #[test]
    fn dict_iteration_layout_for_len_matches_reference_power_of_two_sizing() {
        for len in 0..=256usize {
            let layout = DictIterationLayout::for_len(len);
            let expected_usable = if len == 0 {
                0
            } else {
                let mut table_size = 8usize;
                while super::dict_usable_fraction(table_size) < len {
                    table_size *= 2;
                }
                super::dict_usable_fraction(table_size) - len
            };
            assert_eq!(layout.live_len, len, "len={len}");
            assert_eq!(layout.entry_count, len, "len={len}");
            assert_eq!(layout.usable, expected_usable, "len={len}");
        }
    }
}
