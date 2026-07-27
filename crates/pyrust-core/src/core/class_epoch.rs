use std::cell::Cell;

// Global class-mutation epoch counter.  Bumped on every PyClass attribute write
// or delete, regardless of which class was mutated.  Inline attribute caches
// store the epoch at fill time and re-validate it on each hit; a mismatch means
// some class (possibly an ancestor in the MRO) was mutated since the fill,
// which triggers a cache miss and a slow-path re-fill.
//
// This is the same approach CPython's specialising adaptive interpreter uses to
// invalidate inline caches after class mutations.  `u64::MAX` is a saturated
// sentinel: once reached, class-dependent caches remain disabled rather than
// wrapping to an older stamp and accepting a stale entry (ABA).
thread_local! {
    static CLASS_MUTATION_EPOCH: Cell<u64> = const { Cell::new(0) };
}

/// Bump the global class-mutation epoch.  Call this whenever any `PyClass`
/// attribute is written or deleted so that all attribute caches are invalidated.
pub fn bump_class_epoch() {
    CLASS_MUTATION_EPOCH.with(|c| c.set(c.get().saturating_add(1)));
}

/// Return the current global class-mutation epoch.
pub fn class_epoch() -> u64 {
    CLASS_MUTATION_EPOCH.with(|c| c.get())
}

/// Produce a cacheable `(class version, global epoch)` stamp.
///
/// Either saturated component disables cache fill permanently for that class
/// or host thread.
#[inline]
pub fn class_cache_stamp(class_version: u64) -> Option<(u64, u64)> {
    if class_version == u64::MAX {
        return None;
    }
    let epoch = class_epoch();
    (epoch != u64::MAX).then_some((class_version, epoch))
}

/// Validate a class-dependent cache stamp without permitting saturated values.
#[inline]
pub fn class_cache_stamp_matches(
    current_class_version: u64,
    cached_class_version: u64,
    cached_epoch: u64,
) -> bool {
    cached_class_version != u64::MAX
        && cached_epoch != u64::MAX
        && current_class_version == cached_class_version
        && class_epoch() == cached_epoch
}

#[cfg(test)]
mod tests {
    use super::{
        CLASS_MUTATION_EPOCH, bump_class_epoch, class_cache_stamp, class_cache_stamp_matches,
        class_epoch,
    };

    #[test]
    fn saturated_epoch_never_wraps_or_validates_a_cache_stamp() {
        let original = CLASS_MUTATION_EPOCH.with(|epoch| epoch.replace(u64::MAX - 1));
        bump_class_epoch();
        assert_eq!(class_epoch(), u64::MAX);
        bump_class_epoch();
        assert_eq!(class_epoch(), u64::MAX);
        assert_eq!(class_cache_stamp(7), None);
        assert!(!class_cache_stamp_matches(7, 7, u64::MAX));
        CLASS_MUTATION_EPOCH.with(|epoch| epoch.set(original));
    }
}
