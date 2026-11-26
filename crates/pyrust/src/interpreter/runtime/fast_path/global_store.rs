// `StoreGlobal` root-slot inline cache (#3423).
impl Interpreter {
    /// Execute `StoreGlobal`: overwrite the cached root binding in place, or
    /// run the full assignment and refill the cache.
    ///
    /// The cache only decides *where* a repeated store lands. Whether an
    /// in-place overwrite is equivalent to the full assignment is the
    /// namespace owner's decision, taken again on every hit.
    #[inline]
    pub(super) fn store_global_cached(
        &mut self,
        code: &crate::bytecode::FnCode,
        name_index: u16,
        value: Value,
    ) -> Result<()> {
        let slot = name_index as usize;
        let outcome = match code.global_store_cache.borrow().get(slot) {
            Some(GlobalStoreCacheEntry::RootSlot {
                slot: target,
                declared_in,
            }) => match (
                code.names.get(slot),
                code.global_cache_interest_masks.get(slot),
            ) {
                (Some(name), Some(&name_mask)) => self
                    .overwrite_cached_global_slot(
                        name,
                        name_mask,
                        target,
                        declared_in.as_ref(),
                        &value,
                    )
                    .map(|outcome| (name, outcome)),
                _ => None,
            },
            _ => None,
        };
        match outcome {
            Some((name, outcome)) => {
                self.finish_cached_global_store(name, value, outcome);
                Ok(())
            }
            None => self.store_global_uncached(code, name_index, value),
        }
    }

    /// Full `StoreGlobal`: assign through the namespace owner, then record
    /// whether the same store can be repeated in place.
    #[inline(never)]
    fn store_global_uncached(
        &mut self,
        code: &crate::bytecode::FnCode,
        name_index: u16,
        value: Value,
    ) -> Result<()> {
        let slot = name_index as usize;
        let name = code.names.get(slot).ok_or_else(|| {
            PyError::runtime(format!(
                "bytecode error: name index {name_index} out of range (pool size {})",
                code.names.len()
            ))
        })?;
        self.assign_name(name, value);
        let entry = match self.global_store_cache_slot(name) {
            Some((slot, declared_in)) => GlobalStoreCacheEntry::RootSlot { slot, declared_in },
            None => GlobalStoreCacheEntry::Empty,
        };
        let mut cache = code.global_store_cache.borrow_mut();
        if cache.is_empty() {
            if matches!(entry, GlobalStoreCacheEntry::Empty) {
                // Never allocate the table for a store that cannot be cached.
                return Ok(());
            }
            cache.resize(code.names.len(), GlobalStoreCacheEntry::Empty);
        }
        if let Some(cell) = cache.get_mut(slot) {
            *cell = entry;
        }
        Ok(())
    }
}
