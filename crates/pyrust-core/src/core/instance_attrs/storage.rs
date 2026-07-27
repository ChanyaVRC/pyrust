/// Opaque identity of one concrete Python `member_descriptor` storage cell.
///
/// The visible slot name is presentation metadata, not a physical key:
/// `class B(A): __slots__ = ("x",)` creates a second cell distinct from
/// `A.x`. Native exception fields use the separate name-keyed slot namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemberSlotId(u64);

impl MemberSlotId {
    /// Allocate the identity for a newly-created member descriptor.
    pub fn fresh() -> Self {
        static NEXT_MEMBER_SLOT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_MEMBER_SLOT_ID.fetch_add(1, Ordering::Relaxed);
        assert_ne!(id, 0, "member slot identity space exhausted");
        Self(id)
    }
}

/// Per-instance attribute storage for `PyInstance`.
///
/// CPython shares the attribute-name layout across all instances of a class
/// (PEP 412 key-sharing dicts) and pyrust previously stored a full
/// `IndexMap<String, Value>` per instance — duplicating both the key strings
/// and the hash-table scaffolding in every object (#2012: ~3.15× CPython's
/// per-instance footprint).
///
/// This type replaces that with two contained wins:
///
/// 1. **Interned keys** — names are `Rc<str>` pulled from a per-thread interner
///    ([`intern_attr_key`]), so the bytes for each distinct name are allocated
///    once and shared across every instance, not duplicated per instance.
/// 2. **Compact linear storage** — a plain `Vec<(Rc<str>, Value)>` instead of a
///    hash map.  Instances almost always have a handful of attributes, for
///    which linear scan beats hashing and avoids the IndexMap `RawTable` +
///    capacity overhead entirely.  Insertion order is preserved natively, which
///    CPython requires for `__dict__` iteration.
///
/// **Hybrid index** (#2162): the linear scan in (2) becomes a cold-access cliff
/// for *wide* instances — once an instance carries more than [`INDEX_THRESHOLD`]
/// attributes, a `find()` over the `Vec` costs more than the IndexMap hash
/// lookup it replaced (measured crossover ~16 attrs; 50 attrs ~2.26× slower).
/// To keep the small-instance win while restoring O(1) cold access for wide
/// instances, a side `HashMap<Rc<str>, usize>` (FxHash, name → slot in
/// `entries`) is lazily built once `entries` grows past the threshold.  The
/// `Vec` remains the source of truth for storage and insertion order; the index
/// is a pure lookup accelerator that `get`/`contains_key`/`insert` consult when
/// present, kept in sync on insert and rebuilt on the (rare) shift-remove.
#[derive(Debug, Clone, Default)]
pub struct InstanceAttrs {
    entries: Vec<(Rc<str>, Value)>,
    /// Lazily allocated uncommon storage. Keeping the wide-dict index and
    /// descriptor/native slots behind the same pointer preserves the compact
    /// footprint of plain instances.
    aux: Option<Box<InstanceAttrsAux>>,
    /// When set (issue #1981), the instance's `__dict__` was replaced wholesale
    /// by `obj.__dict__ = d`, and `d` itself is the live backing store.  All
    /// attribute reads/writes route through this real `dict` Value, so
    /// `obj.__dict__ is d` holds and mutations alias both ways (and non-str
    /// keys round-trip). `entries` retains the previous inline backing, while
    /// `aux.native_slots` and `aux.member_slots` remain independent storage. An
    /// `instance_dict` proxy obtained before replacement remains attached to
    /// `entries` just as a real CPython dict does. `None` is the common case.
    dict_ref: Option<Value>,
}

/// Uncommon per-instance state.
///
/// `index` accelerates wide visible dictionaries. `native_slots` stores
/// interpreter-owned named fields such as BaseException's C-style members.
/// `member_slots` stores Python `__slots__` cells by descriptor identity.
/// Both are physically separate from each other and from visible `__dict__`.
#[derive(Debug, Clone, Default)]
struct InstanceAttrsAux {
    index: Option<HashMap<Rc<str>, usize, FxBuildHasher>>,
    native_slots: Vec<(Rc<str>, Value)>,
    member_slots: smallvec::SmallVec<[(MemberSlotId, Value); 2]>,
}

/// Attribute count above which [`InstanceAttrs`] builds a hash index for
/// O(1) cold lookups.  At or below this, linear scan over the compact `Vec`
/// wins (the #2161 small-instance memory + speed gain); the measured crossover
/// versus the old IndexMap hash lookup is ~16 attributes.
const INDEX_THRESHOLD: usize = 16;

impl InstanceAttrs {
    pub fn new() -> Self {
        InstanceAttrs {
            entries: Vec::new(),
            aux: None,
            dict_ref: None,
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        InstanceAttrs {
            entries: Vec::with_capacity(cap),
            aux: None,
            dict_ref: None,
        }
    }

    /// Construct storage for an instance known to populate C-style slots.
    ///
    /// Plain instances use [`Self::new`] and never pay this allocation.
    pub fn with_slot_capacity(cap: usize) -> Self {
        InstanceAttrs {
            entries: Vec::new(),
            aux: Some(Box::new(InstanceAttrsAux {
                index: None,
                native_slots: Vec::with_capacity(cap),
                member_slots: smallvec::SmallVec::new(),
            })),
            dict_ref: None,
        }
    }

    /// True iff this instance's `__dict__` was replaced wholesale (issue #1981),
    /// so attribute storage is a live external `dict` rather than `entries`.
    /// Hot-path callers (VM inline cache) consult this to skip the `entries`
    /// fast path for the rare dict-backed instance.
    #[inline]
    pub fn is_dict_backed(&self) -> bool {
        self.dict_ref.is_some()
    }

    /// Replace the instance `__dict__` with a live `dict` Value (issue #1981).
    /// The dict is stored by reference, so `obj.__dict__ is d` holds and later
    /// mutations alias both ways.
    ///
    /// `entries` is preserved as the backing of any previously returned
    /// `instance_dict` proxy. Descriptor/native slots live separately in
    /// `aux`, so neither old proxies nor the replacement dict can affect them.
    pub fn set_dict_ref(&mut self, dict: Value) {
        self.dict_ref = Some(dict);
    }

    /// Read a C-style slot from storage physically independent of `__dict__`.
    ///
    /// A slot named `x` can therefore coexist with `obj.__dict__["x"]`.
    #[inline]
    pub fn get_slot(&self, name: &str) -> Option<&Value> {
        self.aux
            .as_ref()?
            .native_slots
            .iter()
            .find(|(key, _)| key.as_ref() == name)
            .map(|(_, value)| value)
    }

    /// Mutable C-style slot lookup.
    #[inline]
    pub fn get_slot_mut(&mut self, name: &str) -> Option<&mut Value> {
        self.aux
            .as_mut()?
            .native_slots
            .iter_mut()
            .find(|(key, _)| key.as_ref() == name)
            .map(|(_, value)| value)
    }

    /// Write a C-style slot without touching the Python-visible mapping.
    pub fn insert_slot(&mut self, name: impl AsRef<str>, value: Value) -> Option<Value> {
        let name = name.as_ref();
        let aux = self
            .aux
            .get_or_insert_with(|| Box::new(InstanceAttrsAux::default()));
        if let Some((_, old)) = aux
            .native_slots
            .iter_mut()
            .find(|(key, _)| key.as_ref() == name)
        {
            return Some(std::mem::replace(old, value));
        }
        aux.native_slots.push((intern_attr_key(name), value));
        None
    }

    /// Remove a C-style slot without touching a same-named `__dict__` entry.
    pub fn shift_remove_slot(&mut self, name: &str) -> Option<Value> {
        let aux = self.aux.as_mut()?;
        let pos = aux
            .native_slots
            .iter()
            .position(|(key, _)| key.as_ref() == name)?;
        let removed = aux.native_slots.remove(pos).1;
        self.drop_empty_aux();
        Some(removed)
    }

    /// Number of populated C-style slots. Slots are intentionally absent from
    /// `len()`/`iter()`, which expose only the Python-visible mapping.
    #[inline]
    pub fn slot_len(&self) -> usize {
        self.aux.as_ref().map_or(0, |aux| aux.native_slots.len())
    }

    /// Read one Python member-descriptor cell by its physical identity.
    #[inline(always)]
    pub fn get_member_slot(&self, id: MemberSlotId) -> Option<&Value> {
        self.aux
            .as_ref()?
            .member_slots
            .iter()
            .find(|(slot_id, _)| *slot_id == id)
            .map(|(_, value)| value)
    }

    /// Populate or replace one Python member-descriptor cell.
    pub fn insert_member_slot(&mut self, id: MemberSlotId, value: Value) -> Option<Value> {
        let aux = self
            .aux
            .get_or_insert_with(|| Box::new(InstanceAttrsAux::default()));
        if let Some((_, old)) = aux
            .member_slots
            .iter_mut()
            .find(|(slot_id, _)| *slot_id == id)
        {
            return Some(std::mem::replace(old, value));
        }
        aux.member_slots.push((id, value));
        None
    }

    /// Remove one Python member-descriptor cell.
    pub fn shift_remove_member_slot(&mut self, id: MemberSlotId) -> Option<Value> {
        let aux = self.aux.as_mut()?;
        let pos = aux
            .member_slots
            .iter()
            .position(|(slot_id, _)| *slot_id == id)?;
        let removed = aux.member_slots.remove(pos).1;
        self.drop_empty_aux();
        Some(removed)
    }

    /// Remap locally-declared member cells during compatible `__class__`
    /// reassignment. Inherited descriptor identities are intentionally absent
    /// from `remaps` and remain unchanged.
    pub fn remap_member_slots(&mut self, remaps: &[(MemberSlotId, MemberSlotId)]) {
        let Some(aux) = self.aux.as_mut() else {
            return;
        };
        for (slot_id, _) in &mut aux.member_slots {
            if let Some((_, new_id)) = remaps.iter().find(|(old_id, _)| old_id == slot_id) {
                *slot_id = *new_id;
            }
        }
    }

    /// Number of populated Python member-descriptor cells.
    #[inline]
    pub fn member_slot_len(&self) -> usize {
        self.aux.as_ref().map_or(0, |aux| aux.member_slots.len())
    }

    /// The live backing `dict` Value, when `__dict__` was replaced (#1981).
    #[inline]
    pub fn dict_ref(&self) -> Option<&Value> {
        self.dict_ref.as_ref()
    }

    #[inline]
    pub fn len(&self) -> usize {
        if let Some(d) = &self.dict_ref {
            return d.dict_len().unwrap_or(0);
        }
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        if let Some(d) = &self.dict_ref {
            return d.dict_len().unwrap_or(0) == 0;
        }
        self.entries.is_empty()
    }

    /// Find the slot of `name`, consulting the hash index when one has been
    /// built (wide instance) and falling back to linear scan otherwise.
    #[inline]
    fn position(&self, name: &str) -> Option<usize> {
        if let Some(index) = self.aux.as_ref().and_then(|aux| aux.index.as_ref()) {
            index.get(name).copied()
        } else {
            self.entries.iter().position(|(k, _)| k.as_ref() == name)
        }
    }

    /// Build the side hash index from the current `entries`.  Called once an
    /// instance crosses [`INDEX_THRESHOLD`], and to refresh it after a
    /// shift-remove renumbers slots.
    fn build_index(&mut self) {
        let mut index = HashMap::with_capacity_and_hasher(self.entries.len(), FxBuildHasher);
        for (i, (k, _)) in self.entries.iter().enumerate() {
            index.insert(Rc::clone(k), i);
        }
        self.aux
            .get_or_insert_with(|| Box::new(InstanceAttrsAux::default()))
            .index = Some(index);
    }

    /// Release the uncommon allocation after its final index/slot is removed.
    #[inline]
    fn drop_empty_aux(&mut self) {
        if self.aux.as_ref().is_some_and(|aux| {
            aux.index.is_none() && aux.native_slots.is_empty() && aux.member_slots.is_empty()
        }) {
            self.aux = None;
        }
    }

    /// Look up a value by attribute name, returning a borrow.  Linear scan for
    /// small instances; O(1) hash lookup once the side index has been built
    /// (#2162). This intentionally does not follow `dict_ref`: borrowing into
    /// the external dict's `RefCell` cannot outlive a temporary `Ref`, and the
    /// preserved entries remain the backing of any proxy created before
    /// replacement. Dict-backed attribute callers use [`get_cloned`].
    #[inline]
    pub fn get(&self, name: &str) -> Option<&Value> {
        if let Some(index) = self.aux.as_ref().and_then(|aux| aux.index.as_ref()) {
            return index.get(name).map(|&i| &self.entries[i].1);
        }
        self.entries
            .iter()
            .find(|(k, _)| k.as_ref() == name)
            .map(|(_, v)| v)
    }

    /// Read from the preserved inline mapping without following a replacement
    /// `dict_ref`.
    ///
    /// [`Self::get`] has always had this raw-storage behaviour because it
    /// returns a borrow, but this explicit name documents the backing-identity
    /// boundary at pre-replacement `instance_dict` call sites.
    #[inline]
    pub fn inline_get(&self, name: &str) -> Option<&Value> {
        self.get(name)
    }

    /// Look up a value by attribute name, returning an owned clone.  Routes
    /// through the live `__dict__` when the instance is dict-backed (#1981);
    /// otherwise equivalent to `self.get(name).cloned()`.  Slow attribute-read
    /// paths use this so dict-backed instances resolve attributes correctly.
    #[inline]
    pub fn get_cloned(&self, name: &str) -> Option<Value> {
        if let Some(d) = &self.dict_ref {
            return d.dict_with(|m| m.get(&StrKey(name)).cloned()).flatten();
        }
        self.get(name).cloned()
    }

    /// Read a C-style slot as an owned value.
    ///
    /// Kept as the migration-safe name used by exception machinery. It never
    /// consults `dict_ref` or visible `entries`, so a same-named dict key cannot
    /// shadow the internal slot.
    #[inline]
    pub fn get_cloned_or_slot(&self, name: &str) -> Option<Value> {
        self.get_slot(name).cloned()
    }

    /// Mutable lookup by attribute name.  Lets a caller test-and-replace an
    /// existing visible value with a single key scan. Returns `None` for
    /// dict-backed instances because the external dict's `RefCell` borrow
    /// cannot be returned through this API.
    #[inline]
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Value> {
        if self.dict_ref.is_some() {
            return None;
        }
        let pos = self.position(name)?;
        Some(&mut self.entries[pos].1)
    }

    #[inline]
    pub fn contains_key(&self, name: &str) -> bool {
        if let Some(d) = &self.dict_ref {
            return d
                .dict_with(|m| m.contains_key(&StrKey(name)))
                .unwrap_or(false);
        }
        self.inline_contains_key(name)
    }

    /// Test the preserved inline mapping without following a replacement
    /// `dict_ref`.
    ///
    /// This is the backing-identity boundary used by an `instance_dict` proxy
    /// obtained before `obj.__dict__` was replaced.
    #[inline]
    pub fn inline_contains_key(&self, name: &str) -> bool {
        if let Some(index) = self.aux.as_ref().and_then(|aux| aux.index.as_ref()) {
            return index.contains_key(name);
        }
        self.entries.iter().any(|(k, _)| k.as_ref() == name)
    }

    /// Insert or overwrite `name`'s value, returning the previous value when
    /// the key already existed.  New keys are appended (insertion order
    /// preserved) and interned so the name bytes are shared across instances.
    /// Dict-backed instances (#1981) write straight into the live `__dict__`.
    pub fn insert(&mut self, name: impl AsRef<str>, value: Value) -> Option<Value> {
        let name = name.as_ref();
        if let Some(d) = &self.dict_ref {
            return d.dict_insert(PyKey::str_from(name), value).ok().flatten();
        }
        self.insert_inline(name, value)
    }

    /// Insert into the preserved inline mapping without following a replacement
    /// `dict_ref`.
    ///
    /// Slot descriptors and pre-replacement `instance_dict` proxies are the
    /// intentional callers. New instance attribute writes should use
    /// [`Self::insert`] so they target the live external dict when present.
    pub fn insert_inline(&mut self, name: impl AsRef<str>, value: Value) -> Option<Value> {
        let name = name.as_ref();
        if let Some(pos) = self.position(name) {
            return Some(std::mem::replace(&mut self.entries[pos].1, value));
        }
        let key = intern_attr_key(name);
        let slot = self.entries.len();
        if let Some(index) = self.aux.as_mut().and_then(|aux| aux.index.as_mut()) {
            index.insert(Rc::clone(&key), slot);
        }
        self.entries.push((key, value));
        // Crossing the threshold turns linear scan from the win into the cliff;
        // build the index now so subsequent cold lookups are O(1).
        if self.aux.as_ref().is_none_or(|aux| aux.index.is_none())
            && self.entries.len() > INDEX_THRESHOLD
        {
            self.build_index();
        }
        None
    }

    /// Remove `name`, shifting later entries down to preserve insertion order
    /// (matching `IndexMap::shift_remove`).  Returns the removed value.
    /// Dict-backed instances (#1981) remove from the live `__dict__`.
    pub fn shift_remove(&mut self, name: &str) -> Option<Value> {
        if let Some(d) = &self.dict_ref {
            return d.dict_shift_remove(&PyKey::str_from(name)).ok().flatten();
        }
        self.shift_remove_inline(name)
    }

    /// Remove from the preserved inline mapping without following a
    /// replacement `dict_ref`.
    pub fn shift_remove_inline(&mut self, name: &str) -> Option<Value> {
        let pos = self.position(name)?;
        let removed = self.entries.remove(pos).1;
        // The shift renumbers every entry after `pos`, invalidating the index.
        // Removes are rare; rebuild while the instance is still wide, else drop
        // the index and fall back to linear scan once back at/under threshold.
        if self.aux.as_ref().is_some_and(|aux| aux.index.is_some()) {
            if self.entries.len() > INDEX_THRESHOLD {
                self.build_index();
            } else {
                if let Some(aux) = &mut self.aux {
                    aux.index = None;
                }
                self.drop_empty_aux();
            }
        }
        Some(removed)
    }

    pub fn clear(&mut self) {
        if let Some(d) = &self.dict_ref {
            let _ = d.dict_clear();
            return;
        }
        self.clear_inline();
    }

    /// Clear the preserved inline mapping without following a replacement
    /// `dict_ref`.
    pub fn clear_inline(&mut self) {
        self.entries.clear();
        if let Some(aux) = &mut self.aux {
            aux.index = None;
        }
        self.drop_empty_aux();
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&Rc<str>, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    #[inline]
    pub fn keys(&self) -> impl Iterator<Item = &Rc<str>> {
        self.entries.iter().map(|(k, _)| k)
    }

    /// Number of entries in the preserved inline mapping, without following a
    /// replacement `dict_ref`.
    #[inline]
    pub fn inline_len(&self) -> usize {
        self.entries.len()
    }

    /// Read an inline attribute entry by insertion-order index.
    ///
    /// This deliberately addresses only `entries`. A dict-backed instance
    /// exposes its real `dict` for new lookups, while a proxy created before
    /// replacement remains attached to these preserved entries. The accessor
    /// lets that proxy keep an independent live cursor without restarting a
    /// mapped iterator from index zero.
    #[inline]
    pub fn get_index(&self, index: usize) -> Option<(&Rc<str>, &Value)> {
        self.entries.get(index).map(|(key, value)| (key, value))
    }

    /// Return a preserved inline entry's insertion-order index without
    /// following a replacement `dict_ref`.
    ///
    /// Wide instances use the existing side index; compact instances retain
    /// their bounded linear lookup.
    #[inline]
    pub fn inline_index_of(&self, name: &str) -> Option<usize> {
        self.position(name)
    }

    /// Owned snapshot of `(name, value)` pairs for the string-keyed attributes,
    /// in insertion order. Routes through the live `__dict__` for dict-backed
    /// instances (#1981), where non-str keys are skipped (they are not
    /// attribute-accessible). Attribute-enumeration consumers use this to avoid
    /// the borrow-lifetime constraints of `iter`; a detached `instance_dict`
    /// proxy deliberately reads the inline accessors instead.
    pub fn items_snapshot(&self) -> Vec<(Rc<str>, Value)> {
        if let Some(d) = &self.dict_ref {
            return d
                .dict_with(|m| {
                    m.iter()
                        .filter_map(|(k, v)| match k {
                            PyKey::Str(s) => s.as_str().map(|s| (Rc::<str>::from(s), v.clone())),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        self.entries
            .iter()
            .map(|(k, v)| (Rc::clone(k), v.clone()))
            .collect()
    }
}
