use std::any::TypeId;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use rustc_hash::FxBuildHasher;

// Monotonic identity shared by the container/method representations that do
// not carry a backing allocation address.  `None` is the exhausted state: the
// last `u64` id is returned once, and the next allocation fails loudly instead
// of wrapping to a live object's id.
thread_local! {
    static OBJ_ID_COUNTER: Cell<Option<u64>> = const { Cell::new(Some(1)) };
}

fn take_next_obj_id(counter: &Cell<Option<u64>>) -> u64 {
    let id = counter
        .get()
        .expect("object identity counter exhausted without wrapping");
    counter.set(id.checked_add(1));
    id
}

pub(crate) fn next_obj_id() -> u64 {
    OBJ_ID_COUNTER.with(take_next_obj_id)
}

/// Exact internal identity before it is exposed as Python's numeric `id()`.
///
/// Variants are semantic namespaces, not presentation tags.  Equality is a
/// cheap, allocation-free comparison used by `is`; [`Self::encode`] maps the
/// same key injectively to a bounded non-negative Python integer.  Keeping
/// those two operations on one key makes their equivalence structural rather
/// than a pair of implementations that tests merely sample.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ObjectIdentity {
    /// Address of the live allocation that is the Python object.
    Allocation(u64),
    /// Monotonic identity captured by a representation without a suitable
    /// allocation address (list/tuple/set backing, bound methods, and super).
    Counter(u64),
    /// Complete NaN-box bits for tagged inline values and inline strings.
    RawValue(u64),
    /// Complete untagged float box, including minted NaN identity payloads.
    Float(u64),
    /// Both component bit patterns; pyrust's complex `is` compares these.
    Complex { real: u64, imag: u64 },
    /// A built-in implementation type plus the custom identity payload
    /// supplied by its typed `BuiltinTypeOps::identity_payload` hook.
    Builtin { type_id: TypeId, payload: u64 },
}

/// Largest positive integer that [`Value::int`](crate::Value::int) stores in
/// the NaN-box itself.  Synthetic ids are odd numbers in this range; aligned
/// allocation addresses are even, so the two presentations cannot collide.
const MAX_INLINE_OBJECT_ID: u64 = (1 << 47) - 1;
const FIRST_SYNTHETIC_OBJECT_ID: u64 = 1;

/// Append-only per-thread assignment of typed identities to bounded ids.
///
/// Entries are deliberately never removed and an id is never reassigned to a
/// different typed key.  This gives an identity a stable answer across aliases.
/// If an allocation address is recycled after its old object dies, reusing that
/// key and id is safe because the two objects cannot be observed simultaneously,
/// just as for CPython's address ids.  The cost is O(n) retained metadata for
/// the n distinct non-direct identities observed by `id()` on a thread.
/// `Value` is `!Send + !Sync`, so a thread-local table matches its ownership and
/// avoids synchronization on the call path.
struct BoundedObjectIds {
    ids: HashMap<ObjectIdentity, u64, FxBuildHasher>,
    next: Option<u64>,
}

impl BoundedObjectIds {
    fn new() -> Self {
        Self {
            ids: HashMap::with_hasher(FxBuildHasher),
            next: Some(FIRST_SYNTHETIC_OBJECT_ID),
        }
    }

    fn id_for(&mut self, identity: ObjectIdentity) -> u64 {
        if let Some(id) = self.ids.get(&identity) {
            return *id;
        }

        let id = self
            .next
            .expect("bounded object identity space exhausted without reuse");
        self.next = id
            .checked_add(2)
            .filter(|next| *next <= MAX_INLINE_OBJECT_ID);
        self.ids.insert(identity, id);
        id
    }
}

thread_local! {
    static BOUNDED_OBJECT_IDS: RefCell<BoundedObjectIds> =
        RefCell::new(BoundedObjectIds::new());
}

fn synthetic_object_id(identity: ObjectIdentity) -> u64 {
    BOUNDED_OBJECT_IDS.with(|ids| ids.borrow_mut().id_for(identity))
}

impl ObjectIdentity {
    /// Build an allocation identity from a pointer whose pointee alignment
    /// proves that every possible address is even.  The inline const assertion
    /// is evaluated for every concrete `T`, adding no work to `is` or `id()`.
    #[inline(always)]
    pub(crate) fn allocation_from_ptr<T>(ptr: *const T) -> Self {
        const {
            assert!(
                std::mem::align_of::<T>() >= 2,
                "object identity allocation pointer must be even-aligned"
            );
        }
        debug_assert!(!ptr.is_null());
        Self::Allocation(ptr.addr() as u64)
    }

    /// Build an identity from a pointer already validated by the NaN-box
    /// encoder.  That encoder rejects null, unaligned, and wider-than-48-bit
    /// addresses in release builds before they can enter a `Value`.
    #[inline(always)]
    pub(crate) fn allocation_from_nanbox(address: u64) -> Self {
        debug_assert!(address != 0 && address.is_multiple_of(2));
        Self::Allocation(address)
    }

    /// Encode this typed key as a bounded Python object id.
    ///
    /// Common allocation-backed objects retain their exact address when it is
    /// in the signed 48-bit inline-int range.  All other identities use the
    /// append-only side table.  Allocation addresses are even by construction
    /// while table ids are odd, so neither domain can collide with the other.
    pub(crate) fn encode(self) -> u64 {
        if let Self::Allocation(address) = self {
            assert!(
                address != 0 && address.is_multiple_of(2),
                "object identity allocation address must be non-null and even-aligned"
            );
            if address <= MAX_INLINE_OBJECT_ID {
                return address;
            }
        }
        synthetic_object_id(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{BoundedObjectIds, MAX_INLINE_OBJECT_ID, ObjectIdentity, take_next_obj_id};
    use std::any::TypeId;
    use std::cell::Cell;

    #[test]
    fn typed_identity_keys_receive_stable_distinct_odd_ids() {
        let identities = [
            ObjectIdentity::Counter(7),
            ObjectIdentity::RawValue(7),
            ObjectIdentity::Float(7),
            ObjectIdentity::Complex { real: 7, imag: 0 },
            ObjectIdentity::Builtin {
                type_id: TypeId::of::<u8>(),
                payload: 7,
            },
            ObjectIdentity::Builtin {
                type_id: TypeId::of::<u16>(),
                payload: 7,
            },
        ];
        let mut registry = BoundedObjectIds::new();
        let mut assigned = Vec::new();

        for identity in identities {
            let id = registry.id_for(identity);
            assert_eq!(id % 2, 1);
            assert!(id <= MAX_INLINE_OBJECT_ID);
            assert_eq!(registry.id_for(identity), id);
            assigned.push(id);
        }

        assigned.sort_unstable();
        assigned.dedup();
        assert_eq!(assigned.len(), identities.len());
    }

    #[test]
    fn direct_and_side_table_ids_occupy_disjoint_ranges() {
        let direct_address = 0x0000_1234_5678_9ab8;
        let direct = ObjectIdentity::Allocation(direct_address).encode();
        assert_eq!(direct, direct_address);
        assert_eq!(direct % 2, 0);

        let high_address = MAX_INLINE_OBJECT_ID + 1;
        let bounded = ObjectIdentity::Allocation(high_address).encode();
        assert_eq!(bounded % 2, 1);
        assert!(bounded <= MAX_INLINE_OBJECT_ID);
        assert_ne!(direct, bounded);
        assert_eq!(ObjectIdentity::Allocation(high_address).encode(), bounded);

        let largest_aligned_address = u64::MAX - 1;
        let largest_bounded = ObjectIdentity::Allocation(largest_aligned_address).encode();
        assert_eq!(largest_bounded % 2, 1);
        assert!(largest_bounded <= MAX_INLINE_OBJECT_ID);
        assert_ne!(bounded, largest_bounded);
    }

    #[test]
    #[should_panic(expected = "allocation address must be non-null and even-aligned")]
    fn invalid_odd_allocation_address_is_rejected() {
        ObjectIdentity::Allocation(u64::MAX).encode();
    }

    #[test]
    fn bounded_identity_exhaustion_never_reuses_an_id() {
        let identity = ObjectIdentity::Counter(1);
        let mut registry = BoundedObjectIds::new();
        registry.next = Some(MAX_INLINE_OBJECT_ID);

        assert_eq!(registry.id_for(identity), MAX_INLINE_OBJECT_ID);
        assert_eq!(registry.id_for(identity), MAX_INLINE_OBJECT_ID);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                registry.id_for(ObjectIdentity::Counter(2));
            }))
            .is_err()
        );
    }

    #[test]
    fn monotonic_identity_exhaustion_never_wraps() {
        let counter = Cell::new(Some(u64::MAX));
        assert_eq!(take_next_obj_id(&counter), u64::MAX);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                take_next_obj_id(&counter)
            }))
            .is_err()
        );
    }
}
